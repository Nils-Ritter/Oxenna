//! Block-device abstraction shared by disk drivers and filesystems.
//!
//! All offsets are in 512-byte sectors. Filesystems talk only to this trait,
//! so ATA today can become AHCI / virtio-blk / NVMe later without touching ext2.

use alloc::{boxed::Box, vec::Vec};

pub const SECTOR_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// LBA range is beyond the end of the device.
    OutOfRange,
    /// Buffer length is not a multiple of the sector size.
    BadBuffer,
    /// Device did not respond in time.
    Timeout,
    /// Device reported an error (ERR / DF status bits).
    DeviceFault,
}

pub trait BlockDevice {
    fn sector_count(&self) -> u64;
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    /// Force the device write cache to stable storage.
    fn flush(&mut self) -> Result<(), BlockError>;
}

/// Lets the VFS hold `Ext2<Box<dyn BlockDevice>>` without generics everywhere.
impl BlockDevice for Box<dyn BlockDevice> {
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        (**self).read_sectors(lba, buf)
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        (**self).write_sectors(lba, buf)
    }
    fn flush(&mut self) -> Result<(), BlockError> {
        (**self).flush()
    }
}

/// RAM-backed disk. Useful for `make test` (no QEMU disk needed) and host tools.
pub struct RamDisk {
    data: Vec<u8>,
}

impl RamDisk {
    pub fn new(sectors: usize) -> Self {
        Self { data: alloc::vec![0u8; sectors * SECTOR_SIZE] }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }
    fn range(&self, lba: u64, len: usize) -> Result<core::ops::Range<usize>, BlockError> {
        if len % SECTOR_SIZE != 0 {
            return Err(BlockError::BadBuffer);
        }
        let start = (lba as usize).checked_mul(SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
        let end = start.checked_add(len).ok_or(BlockError::OutOfRange)?;
        if end > self.data.len() {
            return Err(BlockError::OutOfRange);
        }
        Ok(start..end)
    }
}

impl BlockDevice for RamDisk {
    fn sector_count(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let r = self.range(lba, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let r = self.range(lba, buf.len())?;
        self.data[r].copy_from_slice(buf);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}
