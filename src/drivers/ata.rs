//! ATA PIO driver (legacy IDE, LBA28 + LBA48), polling only.
//!
//! Why PIO: it needs no PCI enumeration, no DMA buffers and no IRQ handler,
//! and QEMU's default `pc` (i440FX) machine exposes it at the legacy ports.
//! It is slow (a few MB/s) but simple and robust; an AHCI/virtio driver can
//! later implement the same `BlockDevice` trait.
//!
//! NOTE: QEMU's `-M q35` has no legacy IDE (SATA/AHCI only), so this driver finds nothing there. Use the default machine or `-M pc`.

use super::block::{BlockDevice, BlockError, SECTOR_SIZE};
use alloc::{string::String, vec::Vec};
use core::arch::asm;

const REG_DATA: u16 = 0;
const REG_SECCOUNT: u16 = 2;
const REG_LBA_LO: u16 = 3;
const REG_LBA_MID: u16 = 4;
const REG_LBA_HI: u16 = 5;
const REG_DRIVE: u16 = 6;
const REG_STATUS: u16 = 7; // read: status, write: command

const ST_ERR: u8 = 0x01;
const ST_DRQ: u8 = 0x08;
const ST_DF: u8 = 0x20;
const ST_BSY: u8 = 0x80;

const CMD_READ28: u8 = 0x20;
const CMD_READ48: u8 = 0x24;
const CMD_WRITE28: u8 = 0x30;
const CMD_WRITE48: u8 = 0x34;
const CMD_FLUSH28: u8 = 0xE7;
const CMD_FLUSH48: u8 = 0xEA;
const CMD_IDENTIFY: u8 = 0xEC;

const MAX_SECTORS_PER_CMD: usize = 128;
const SPIN_LIMIT: u32 = 5_000_000;

#[inline]
unsafe fn outb(port: u16, v: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) }
}
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) }
    v
}
#[inline]
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    unsafe { asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags)) }
    v
}
#[inline]
unsafe fn outw(port: u16, v: u16) {
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) }
}

pub struct AtaDrive {
    io: u16,
    ctl: u16,
    slave: bool,
    lba48: bool,
    sectors: u64,
    model: String,
}

impl AtaDrive {
    /// (io base, control base) of the legacy channels.
    pub const PRIMARY: (u16, u16) = (0x1F0, 0x3F6);
    pub const SECONDARY: (u16, u16) = (0x170, 0x376);

    /// Probe one drive slot with IDENTIFY. Returns `None` if nothing (or an
    /// ATAPI device such as a CD-ROM) is present.
    ///
    /// # Safety
    /// Performs raw port I/O; call once during boot with no concurrent access.
    pub unsafe fn probe(io: u16, ctl: u16, slave: bool) -> Option<AtaDrive> {
        let mut d = AtaDrive { io, ctl, slave, lba48: false, sectors: 0, model: String::new() };
        unsafe {
            outb(ctl, 0x02); // nIEN: we poll, never raise IRQ14/15
            outb(io + REG_DRIVE, 0xA0 | ((slave as u8) << 4));
            d.delay();
            for r in [REG_SECCOUNT, REG_LBA_LO, REG_LBA_MID, REG_LBA_HI] {
                outb(io + r, 0);
            }
            outb(io + REG_STATUS, CMD_IDENTIFY);
            d.delay();
            let s = inb(io + REG_STATUS);
            if s == 0 || s == 0xFF {
                return None; // no device / floating bus
            }
            if s & ST_ERR != 0 {
                return None; // not an ATA disk (e.g. ATAPI)
            }
            d.wait_not_busy().ok()?;
            if inb(io + REG_LBA_MID) != 0 || inb(io + REG_LBA_HI) != 0 {
                return None; // ATAPI or SATA signature: not a plain ATA disk
            }
            d.wait_drq().ok()?;
            let mut w = [0u16; 256];
            for x in w.iter_mut() {
                *x = inw(io + REG_DATA);
            }
            if w[49] & (1 << 9) == 0 {
                return None; // no LBA support
            }
            let lba28 = (w[60] as u64) | ((w[61] as u64) << 16);
            let lba48_sectors =
                (w[100] as u64) | ((w[101] as u64) << 16) | ((w[102] as u64) << 32) | ((w[103] as u64) << 48);
            if w[83] & (1 << 14) != 0
                && w[83] & (1 << 10) != 0
                && lba48_sectors != 0
            {
                d.lba48 = true;
                d.sectors = lba48_sectors;
            } else {
                d.sectors = lba28;
            }
            let mut model = Vec::new();
            for &x in &w[27..47] {
                model.push((x >> 8) as u8);
                model.push(x as u8);
            }
            d.model = String::from_utf8_lossy(&model).trim().into();
        }
        if d.sectors == 0 { None } else { Some(d) }
    }

    /// Probe all four legacy slots (primary/secondary x master/slave), in that order.
    ///
    /// # Safety
    /// See [`AtaDrive::probe`].
    pub unsafe fn probe_all() -> Vec<AtaDrive> {
        let mut v = Vec::new();
        for (io, ctl) in [Self::PRIMARY, Self::SECONDARY] {
            for slave in [false, true] {
                if let Some(d) = unsafe { Self::probe(io, ctl, slave) } {
                    v.push(d);
                }
            }
        }
        v
    }

    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn is_lba48(&self) -> bool {
        self.lba48
    }
    pub fn location(&self) -> (&'static str, &'static str) {
        (if self.io == 0x1F0 { "primary" } else { "secondary" }, if self.slave { "slave" } else { "master" })
    }

    // ---- low level helpers -------------------------------------------------

    /// ~400ns: four reads of the alternate status register.
    fn delay(&self) {
        for _ in 0..4 {
            unsafe { inb(self.ctl) };
        }
    }

    fn wait_not_busy(&self) -> Result<u8, BlockError> {
        for _ in 0..SPIN_LIMIT {
            let s = unsafe { inb(self.io + REG_STATUS) };
            if s & ST_BSY == 0 {
                if s & (ST_ERR | ST_DF) != 0 {
                    return Err(BlockError::DeviceFault);
                }
                return Ok(s);
            }
        }
        Err(BlockError::Timeout)
    }

    fn wait_drq(&self) -> Result<(), BlockError> {
        for _ in 0..SPIN_LIMIT {
            let s = unsafe { inb(self.io + REG_STATUS) };
            if s & ST_BSY == 0 {
                if s & (ST_ERR | ST_DF) != 0 {
                    return Err(BlockError::DeviceFault);
                }
                if s & ST_DRQ != 0 {
                    return Ok(());
                }
            }
        }
        Err(BlockError::Timeout)
    }

    fn issue(&mut self, lba: u64, count: usize, write: bool) -> Result<(), BlockError> {
        if count == 0 || count > 255 {
            return Err(BlockError::BadBuffer);
        }

        if self.lba48 {
            if lba > 0x0000_FFFF_FFFF_FFFF {
                return Err(BlockError::OutOfRange);
            }
        } else if lba > 0x0FFF_FFFF {
            return Err(BlockError::OutOfRange);
        }

        let drv = (self.slave as u8) << 4;
        unsafe {
            if self.lba48 {
                outb(self.io + REG_DRIVE, 0x40 | drv);
            } else {
                outb(self.io + REG_DRIVE, 0xE0 | drv | ((lba >> 24) & 0x0F) as u8);
            }
            self.delay();
            self.wait_not_busy()?;
            let cmd;
            if self.lba48 {
                outb(self.io + REG_SECCOUNT, (count >> 8) as u8);
                outb(self.io + REG_LBA_LO, (lba >> 24) as u8);
                outb(self.io + REG_LBA_MID, (lba >> 32) as u8);
                outb(self.io + REG_LBA_HI, (lba >> 40) as u8);
                outb(self.io + REG_SECCOUNT, count as u8);
                outb(self.io + REG_LBA_LO, lba as u8);
                outb(self.io + REG_LBA_MID, (lba >> 8) as u8);
                outb(self.io + REG_LBA_HI, (lba >> 16) as u8);
                cmd = if write { CMD_WRITE48 } else { CMD_READ48 };
            } else {
                outb(self.io + REG_SECCOUNT, count as u8);
                outb(self.io + REG_LBA_LO, lba as u8);
                outb(self.io + REG_LBA_MID, (lba >> 8) as u8);
                outb(self.io + REG_LBA_HI, (lba >> 16) as u8);
                cmd = if write { CMD_WRITE28 } else { CMD_READ28 };
            }
            outb(self.io + REG_STATUS, cmd);
        }
        self.delay();
        Ok(())
    }

    fn check(&self, lba: u64, len: usize) -> Result<usize, BlockError> {
        if len % SECTOR_SIZE != 0 {
            return Err(BlockError::BadBuffer);
        }
        let n = len / SECTOR_SIZE;
        if lba.checked_add(n as u64).map_or(true, |e| e > self.sectors) {
            return Err(BlockError::OutOfRange);
        }
        Ok(n)
    }
}

impl BlockDevice for AtaDrive {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let total = self.check(lba, buf.len())?;
        let mut done = 0usize;
        while done < total {
            let n = (total - done).min(MAX_SECTORS_PER_CMD);
            self.issue(lba + done as u64, n, false)?;
            for s in 0..n {
                self.wait_drq()?;
                let base = (done + s) * SECTOR_SIZE;
                for i in 0..SECTOR_SIZE / 2 {
                    let w = unsafe { inw(self.io + REG_DATA) };
                    buf[base + i * 2] = w as u8;
                    buf[base + i * 2 + 1] = (w >> 8) as u8;
                }
            }
            done += n;
        }
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let total = self.check(lba, buf.len())?;
        let mut done = 0usize;
        while done < total {
            let n = (total - done).min(MAX_SECTORS_PER_CMD);
            self.issue(lba + done as u64, n, true)?;
            for s in 0..n {
                self.wait_drq()?;
                let base = (done + s) * SECTOR_SIZE;
                for i in 0..SECTOR_SIZE / 2 {
                    let w = (buf[base + i * 2] as u16) | ((buf[base + i * 2 + 1] as u16) << 8);
                    unsafe { outw(self.io + REG_DATA, w) };
                }
            }
            let st = self.wait_not_busy()?;
            if st & (ST_ERR | ST_DF) != 0 {
                return Err(BlockError::DeviceFault);
            }
            done += n;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        unsafe {
            outb(self.io + REG_DRIVE, 0xE0 | ((self.slave as u8) << 4));
            self.delay();
            self.wait_not_busy()?;
            outb(self.io + REG_STATUS, if self.lba48 { CMD_FLUSH48 } else { CMD_FLUSH28 });
        }
        self.delay();
        let st = self.wait_not_busy()?;
        if st & (ST_ERR | ST_DF) != 0 { Err(BlockError::DeviceFault) } else { Ok(()) }
    }
}
