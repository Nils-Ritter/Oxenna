//! ext2 filesystem (read/write, format, symlinks, hard links, rename).
//!
//! Why ext2: real POSIX semantics (symlinks, hard links, modes, uid/gid,
//! device nodes), a simple on-disk layout, no journal to implement, and the
//! host's `mke2fs`/`e2fsck`/`debugfs`/`mount` can create, verify and inspect
//! images, which makes testing your driver easy.
//!
//! The API is inode-number based (like a VFS node layer): `lookup`, `read`,
//! `write`, `create`, `mkdir`, `symlink`, `unlink`, ... plus path helpers.
//!
//! Supported: 1K/2K/4K blocks, rev 0/1, FILETYPE + SPARSE_SUPER (+LARGE_FILE
//! images are mounted, but files are capped at 2 GiB - 1).
//! Not supported: journal (ext3), extents/64bit/flex_bg/metadata_csum (ext4),
//! htree write support (the INDEX flag is cleared on modification),
//! extended attributes, block cache, crash consistency (run e2fsck after an
//! unclean shutdown).

use crate::drivers::block::{BlockDevice, BlockError, SECTOR_SIZE};
use alloc::{string::String, vec, vec::Vec};
use core::cmp::min;

pub mod mkfs;
pub mod selftest;

pub use mkfs::FormatOptions;

pub const ROOT_INO: u32 = 2;

const MAGIC: u16 = 0xEF53;
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;
const S_IFLNK: u16 = 0xA000;

const FT_REG: u8 = 1;
const FT_DIR: u8 = 2;
const FT_SYMLINK: u8 = 7;

const INCOMPAT_FILETYPE: u32 = 0x2;
const RO_COMPAT_SPARSE_SUPER: u32 = 0x1;
const RO_COMPAT_LARGE_FILE: u32 = 0x2;
const INDEX_FL: u32 = 0x1000;

const MAX_NAME: usize = 255;
const MAX_FILE_SIZE: u64 = 0x7FFF_FFFF;
const MAX_LINKS: u16 = 32000;
const MAX_SYMLOOP: u32 = 40;
const FAST_SYMLINK_MAX: usize = 60;

// ---------------------------------------------------------------- errors ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io(BlockError),
    BadSuperblock,
    Unsupported,
    Corrupt,
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    Invalid,
    NameTooLong,
    TooManyLinks,
    TooBig,
    Loop,
}

impl From<BlockError> for FsError {
    fn from(e: BlockError) -> Self {
        FsError::Io(e)
    }
}

impl FsError {
    /// Linux errno value, for your syscall layer.
    pub fn errno(self) -> i32 {
        match self {
            FsError::Io(_) | FsError::Corrupt | FsError::BadSuperblock => 5, // EIO
            FsError::NotFound => 2,
            FsError::Exists => 17,
            FsError::NotDir => 20,
            FsError::IsDir => 21,
            FsError::NotEmpty => 39,
            FsError::NoSpace => 28,
            FsError::Invalid | FsError::Unsupported => 22,
            FsError::NameTooLong => 36,
            FsError::TooManyLinks => 31,
            FsError::TooBig => 27,
            FsError::Loop => 40,
        }
    }
}

// --------------------------------------------------------------- helpers ----

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn wr16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn align4(n: usize) -> usize {
    (n + 3) & !3
}
fn bit_test(b: &[u8], i: usize) -> bool {
    b[i / 8] & (1 << (i % 8)) != 0
}
fn bit_set(b: &mut [u8], i: usize) {
    b[i / 8] |= 1 << (i % 8);
}
fn bit_clear(b: &mut [u8], i: usize) {
    b[i / 8] &= !(1 << (i % 8));
}
fn bit_find_zero(b: &[u8], n: usize) -> Option<usize> {
    for byte in 0..(n + 7) / 8 {
        if b[byte] != 0xFF {
            for bit in 0..8 {
                let i = byte * 8 + bit;
                if i >= n {
                    return None;
                }
                if !bit_test(b, i) {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// With SPARSE_SUPER only groups 0, 1 and powers of 3, 5, 7 hold backups.
pub(crate) fn has_super(g: u32) -> bool {
    if g <= 1 {
        return true;
    }
    if g % 2 == 0 {
        return false;
    }
    for base in [3u64, 5, 7] {
        let mut n = base;
        while n < g as u64 {
            n *= base;
        }
        if n == g as u64 {
            return true;
        }
    }
    false
}

fn validate_name(name: &[u8]) -> Result<(), FsError> {
    if name.is_empty() || name.contains(&0) || name.contains(&b'/') {
        return Err(FsError::Invalid);
    }
    if name.len() > MAX_NAME {
        return Err(FsError::NameTooLong);
    }
    Ok(())
}

// ----------------------------------------------------------------- types ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Unknown,
}

impl FileType {
    fn from_mode(m: u16) -> Self {
        match m & S_IFMT {
            S_IFREG => FileType::Regular,
            S_IFDIR => FileType::Directory,
            S_IFLNK => FileType::Symlink,
            0x2000 => FileType::CharDevice,
            0x6000 => FileType::BlockDevice,
            0x1000 => FileType::Fifo,
            0xC000 => FileType::Socket,
            _ => FileType::Unknown,
        }
    }
    fn from_dirent(t: u8) -> Self {
        match t {
            1 => FileType::Regular,
            2 => FileType::Directory,
            3 => FileType::CharDevice,
            4 => FileType::BlockDevice,
            5 => FileType::Fifo,
            6 => FileType::Socket,
            7 => FileType::Symlink,
            _ => FileType::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Stat {
    pub ino: u32,
    /// Full st_mode (type bits + permissions).
    pub mode: u16,
    pub file_type: FileType,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub links: u16,
    /// 512-byte units, like st_blocks.
    pub blocks: u32,
    pub atime: u32,
    pub mtime: u32,
    pub ctime: u32,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub ino: u32,
    pub name: String,
    pub file_type: FileType,
}

#[derive(Debug, Clone, Copy)]
pub struct FsUsage {
    pub block_size: u32,
    pub total_blocks: u32,
    pub free_blocks: u32,
    pub total_inodes: u32,
    pub free_inodes: u32,
}

/// Raw 128-byte on-disk inode. Kept as bytes so unknown fields survive rewrites.
#[derive(Clone, Copy)]
struct Inode {
    raw: [u8; 128],
}

impl Inode {
    fn zeroed() -> Self {
        Inode { raw: [0; 128] }
    }
    fn mode(&self) -> u16 { rd16(&self.raw, 0) }
    fn set_mode(&mut self, v: u16) { wr16(&mut self.raw, 0, v) }
    fn size(&self) -> u32 { rd32(&self.raw, 4) }
    fn set_size(&mut self, v: u32) { wr32(&mut self.raw, 4, v) }
    fn atime(&self) -> u32 { rd32(&self.raw, 8) }
    fn set_atime(&mut self, v: u32) { wr32(&mut self.raw, 8, v) }
    fn ctime(&self) -> u32 { rd32(&self.raw, 12) }
    fn set_ctime(&mut self, v: u32) { wr32(&mut self.raw, 12, v) }
    fn mtime(&self) -> u32 { rd32(&self.raw, 16) }
    fn set_mtime(&mut self, v: u32) { wr32(&mut self.raw, 16, v) }
    fn set_dtime(&mut self, v: u32) { wr32(&mut self.raw, 20, v) }
    fn links(&self) -> u16 { rd16(&self.raw, 26) }
    fn set_links(&mut self, v: u16) { wr16(&mut self.raw, 26, v) }
    fn blocks(&self) -> u32 { rd32(&self.raw, 28) }
    fn set_blocks(&mut self, v: u32) { wr32(&mut self.raw, 28, v) }
    fn flags(&self) -> u32 { rd32(&self.raw, 32) }
    fn set_flags(&mut self, v: u32) { wr32(&mut self.raw, 32, v) }
    fn block(&self, i: usize) -> u32 { rd32(&self.raw, 40 + 4 * i) }
    fn set_block(&mut self, i: usize, v: u32) { wr32(&mut self.raw, 40 + 4 * i, v) }
    fn uid(&self) -> u32 { rd16(&self.raw, 2) as u32 | ((rd16(&self.raw, 120) as u32) << 16) }
    fn gid(&self) -> u32 { rd16(&self.raw, 24) as u32 | ((rd16(&self.raw, 122) as u32) << 16) }
    fn kind(&self) -> u16 { self.mode() & S_IFMT }
    fn is_dir(&self) -> bool { self.kind() == S_IFDIR }
    fn is_reg(&self) -> bool { self.kind() == S_IFREG }
    fn is_lnk(&self) -> bool { self.kind() == S_IFLNK }
    fn is_fast_symlink(&self) -> bool { self.is_lnk() && (self.size() as usize) < FAST_SYMLINK_MAX }
    /// Does i_block[] hold block pointers (as opposed to inline symlink text / device numbers)?
    fn has_block_pointers(&self) -> bool {
        self.is_reg() || self.is_dir() || (self.is_lnk() && !self.is_fast_symlink())
    }
}

struct RawDirent {
    inode: u32,
    rec_len: usize,
    name_len: usize,
    ftype: u8,
}

fn parse_dirent(b: &[u8], off: usize, bs: usize) -> Result<RawDirent, FsError> {
    if off + 8 > bs {
        return Err(FsError::Corrupt);
    }
    let rec_len = rd16(b, off + 4) as usize;
    let name_len = b[off + 6] as usize;
    if rec_len < 8 || rec_len % 4 != 0 || off + rec_len > bs || 8 + name_len > rec_len {
        return Err(FsError::Corrupt);
    }
    Ok(RawDirent { inode: rd32(b, off), rec_len, name_len, ftype: b[off + 7] })
}

fn put_dirent(buf: &mut [u8], off: usize, ino: u32, rec_len: usize, name: &[u8], ft: u8) {
    wr32(buf, off, ino);
    wr16(buf, off + 4, rec_len as u16);
    buf[off + 6] = name.len() as u8;
    buf[off + 7] = ft;
    buf[off + 8..off + 8 + name.len()].copy_from_slice(name);
}

fn zero_clock() -> u32 {
    0
}

// ------------------------------------------------------------ filesystem ----

pub struct Ext2<D: BlockDevice> {
    dev: D,
    bs: usize,
    spb: u32, // sectors per block
    inodes_count: u32,
    blocks_count: u32,
    free_blocks: u32,
    free_inodes: u32,
    first_data_block: u32,
    bpg: u32,
    ipg: u32,
    inode_size: usize,
    first_ino: u32,
    ngroups: u32,
    filetype: bool,
    gdt: Vec<u8>,
    sb: [u8; 1024],
    dirty: bool,
    clock: fn() -> u32,
}

impl<D: BlockDevice> Ext2<D> {
    // ---- mount / unmount ---------------------------------------------------

    pub fn mount(mut dev: D) -> Result<Self, FsError> {
        let mut sb = [0u8; 1024];
        dev.read_sectors(2, &mut sb)?; // superblock lives at byte offset 1024
        if rd16(&sb, 56) != MAGIC {
            return Err(FsError::BadSuperblock);
        }
        let log = rd32(&sb, 24);
        if log > 2 {
            return Err(FsError::Unsupported);
        }
        let bs = 1024usize << log;
        let rev = rd32(&sb, 76);
        let (inode_size, first_ino, filetype) = if rev >= 1 {
            let incompat = rd32(&sb, 96);
            let ro = rd32(&sb, 100);
            if incompat & !INCOMPAT_FILETYPE != 0 || ro & !(RO_COMPAT_SPARSE_SUPER | RO_COMPAT_LARGE_FILE) != 0 {
                return Err(FsError::Unsupported);
            }
            (rd16(&sb, 88) as usize, rd32(&sb, 84), incompat & INCOMPAT_FILETYPE != 0)
        } else {
            (128, 11, false)
        };
        if inode_size < 128 || inode_size > bs || !inode_size.is_power_of_two() {
            return Err(FsError::Unsupported);
        }
        let blocks_count = rd32(&sb, 4);
        let first_data_block = rd32(&sb, 20);
        let bpg = rd32(&sb, 32);
        let ipg = rd32(&sb, 40);
        if bpg == 0 || ipg == 0 || bpg as usize > bs * 8 || ipg as usize > bs * 8 || blocks_count <= first_data_block {
            return Err(FsError::BadSuperblock);
        }
        let spb = (bs / SECTOR_SIZE) as u32;
        if (blocks_count as u64) * spb as u64 > dev.sector_count() {
            return Err(FsError::Corrupt);
        }
        let ngroups = (blocks_count - first_data_block + bpg - 1) / bpg;
        let inodes_count = rd32(&sb, 0);
        if inodes_count as u64 != ngroups as u64 * ipg as u64 {
            return Err(FsError::BadSuperblock);
        }
        let gdt_blocks = ((ngroups as usize) * 32 + bs - 1) / bs;
        let mut gdt = vec![0u8; gdt_blocks * bs];
        dev.read_sectors((first_data_block as u64 + 1) * spb as u64, &mut gdt)?;

        Ok(Ext2 {
            dev,
            bs,
            spb,
            inodes_count,
            blocks_count,
            free_blocks: rd32(&sb, 12),
            free_inodes: rd32(&sb, 16),
            first_data_block,
            bpg,
            ipg,
            inode_size,
            first_ino,
            ngroups,
            filetype,
            gdt,
            sb,
            dirty: false,
            clock: zero_clock,
        })
    }

    /// Provide a wall-clock source (seconds since the Unix epoch), e.g. from the RTC.
    pub fn set_clock(&mut self, f: fn() -> u32) {
        self.clock = f;
    }

    /// Flush all metadata and the device cache. Call before shutdown/`exit`.
    pub fn sync(&mut self) -> Result<(), FsError> {
        if self.dirty {
            let now = (self.clock)();
            wr32(&mut self.sb, 12, self.free_blocks);
            wr32(&mut self.sb, 16, self.free_inodes);
            wr32(&mut self.sb, 48, now);
            self.dev.write_sectors(2, &self.sb)?;
            let lba = (self.first_data_block as u64 + 1) * self.spb as u64;
            self.dev.write_sectors(lba, &self.gdt)?;
            self.dirty = false;
        }
        self.dev.flush()?;
        Ok(())
    }

    pub fn into_device(mut self) -> Result<D, FsError> {
        self.sync()?;
        Ok(self.dev)
    }

    pub fn usage(&self) -> FsUsage {
        FsUsage {
            block_size: self.bs as u32,
            total_blocks: self.blocks_count,
            free_blocks: self.free_blocks,
            total_inodes: self.inodes_count,
            free_inodes: self.free_inodes,
        }
    }

    // ---- raw block / inode access -------------------------------------------

    fn read_block(&mut self, blk: u32, buf: &mut [u8]) -> Result<(), FsError> {
        if blk >= self.blocks_count {
            return Err(FsError::Corrupt);
        }
        self.dev.read_sectors(blk as u64 * self.spb as u64, &mut buf[..self.bs])?;
        Ok(())
    }

    fn write_block(&mut self, blk: u32, buf: &[u8]) -> Result<(), FsError> {
        if blk >= self.blocks_count {
            return Err(FsError::Corrupt);
        }
        self.dev.write_sectors(blk as u64 * self.spb as u64, &buf[..self.bs])?;
        Ok(())
    }

    fn gd_off(g: u32) -> usize {
        g as usize * 32
    }
    fn gd_block_bitmap(&self, g: u32) -> u32 { rd32(&self.gdt, Self::gd_off(g)) }
    fn gd_inode_bitmap(&self, g: u32) -> u32 { rd32(&self.gdt, Self::gd_off(g) + 4) }
    fn gd_inode_table(&self, g: u32) -> u32 { rd32(&self.gdt, Self::gd_off(g) + 8) }
    fn gd_free_blocks(&self, g: u32) -> u16 { rd16(&self.gdt, Self::gd_off(g) + 12) }
    fn gd_free_inodes(&self, g: u32) -> u16 { rd16(&self.gdt, Self::gd_off(g) + 14) }
    fn gd_used_dirs(&self, g: u32) -> u16 { rd16(&self.gdt, Self::gd_off(g) + 16) }
    fn gd_set_free_blocks(&mut self, g: u32, v: u16) { wr16(&mut self.gdt, Self::gd_off(g) + 12, v) }
    fn gd_set_free_inodes(&mut self, g: u32, v: u16) { wr16(&mut self.gdt, Self::gd_off(g) + 14, v) }
    fn gd_set_used_dirs(&mut self, g: u32, v: u16) { wr16(&mut self.gdt, Self::gd_off(g) + 16, v) }

    fn inode_loc(&self, ino: u32) -> Result<(u32, usize), FsError> {
        if ino == 0 || ino > self.inodes_count {
            return Err(FsError::Invalid);
        }
        let g = (ino - 1) / self.ipg;
        let idx = (ino - 1) % self.ipg;
        let byte = idx as u64 * self.inode_size as u64;
        Ok((self.gd_inode_table(g) + (byte / self.bs as u64) as u32, (byte % self.bs as u64) as usize))
    }

    fn read_inode(&mut self, ino: u32) -> Result<Inode, FsError> {
        let (blk, off) = self.inode_loc(ino)?;
        let mut buf = vec![0u8; self.bs];
        self.read_block(blk, &mut buf)?;
        let mut i = Inode::zeroed();
        i.raw.copy_from_slice(&buf[off..off + 128]);
        Ok(i)
    }

    fn write_inode(&mut self, ino: u32, i: &Inode) -> Result<(), FsError> {
        let (blk, off) = self.inode_loc(ino)?;
        let mut buf = vec![0u8; self.bs];
        self.read_block(blk, &mut buf)?;
        buf[off..off + 128].copy_from_slice(&i.raw);
        self.write_block(blk, &buf)
    }

    // ---- allocation -----------------------------------------------------------

    fn alloc_block(&mut self, goal: u32) -> Result<u32, FsError> {
        for k in 0..self.ngroups {
            let g = (goal + k) % self.ngroups;
            if self.gd_free_blocks(g) == 0 {
                continue;
            }
            let bb = self.gd_block_bitmap(g);
            let mut bm = vec![0u8; self.bs];
            self.read_block(bb, &mut bm)?;
            if let Some(bit) = bit_find_zero(&bm, self.bpg as usize) {
                let blk = self.first_data_block + g * self.bpg + bit as u32;
                if blk >= self.blocks_count {
                    return Err(FsError::Corrupt);
                }
                bit_set(&mut bm, bit);
                self.write_block(bb, &bm)?;
                let f = self.gd_free_blocks(g);
                self.gd_set_free_blocks(g, f - 1);
                self.free_blocks -= 1;
                self.dirty = true;
                return Ok(blk);
            }
        }
        Err(FsError::NoSpace)
    }

    fn alloc_zeroed_block(&mut self, goal: u32) -> Result<u32, FsError> {
        let b = self.alloc_block(goal)?;
        let z = vec![0u8; self.bs];
        self.write_block(b, &z)?;
        Ok(b)
    }

    fn free_block(&mut self, blk: u32) -> Result<(), FsError> {
        if blk < self.first_data_block || blk >= self.blocks_count {
            return Err(FsError::Corrupt);
        }
        let rel = blk - self.first_data_block;
        let g = rel / self.bpg;
        let bit = (rel % self.bpg) as usize;
        let bb = self.gd_block_bitmap(g);
        let mut bm = vec![0u8; self.bs];
        self.read_block(bb, &mut bm)?;
        if !bit_test(&bm, bit) {
            return Err(FsError::Corrupt); // double free
        }
        bit_clear(&mut bm, bit);
        self.write_block(bb, &bm)?;
        let f = self.gd_free_blocks(g);
        self.gd_set_free_blocks(g, f + 1);
        self.free_blocks += 1;
        self.dirty = true;
        Ok(())
    }

    fn alloc_inode(&mut self, goal: u32, is_dir: bool) -> Result<u32, FsError> {
        for k in 0..self.ngroups {
            let g = (goal + k) % self.ngroups;
            if self.gd_free_inodes(g) == 0 {
                continue;
            }
            let ib = self.gd_inode_bitmap(g);
            let mut bm = vec![0u8; self.bs];
            self.read_block(ib, &mut bm)?;
            if let Some(bit) = bit_find_zero(&bm, self.ipg as usize) {
                bit_set(&mut bm, bit);
                self.write_block(ib, &bm)?;
                let f = self.gd_free_inodes(g);
                self.gd_set_free_inodes(g, f - 1);
                if is_dir {
                    let d = self.gd_used_dirs(g);
                    self.gd_set_used_dirs(g, d + 1);
                }
                self.free_inodes -= 1;
                self.dirty = true;
                return Ok(g * self.ipg + bit as u32 + 1);
            }
        }
        Err(FsError::NoSpace)
    }

    fn free_inode(&mut self, ino: u32, was_dir: bool) -> Result<(), FsError> {
        if ino < self.first_ino || ino > self.inodes_count {
            return Err(FsError::Invalid);
        }
        let g = (ino - 1) / self.ipg;
        let bit = ((ino - 1) % self.ipg) as usize;
        let ib = self.gd_inode_bitmap(g);
        let mut bm = vec![0u8; self.bs];
        self.read_block(ib, &mut bm)?;
        if !bit_test(&bm, bit) {
            return Err(FsError::Corrupt);
        }
        bit_clear(&mut bm, bit);
        self.write_block(ib, &bm)?;
        let f = self.gd_free_inodes(g);
        self.gd_set_free_inodes(g, f + 1);
        if was_dir {
            let d = self.gd_used_dirs(g);
            self.gd_set_used_dirs(g, d.saturating_sub(1));
        }
        self.free_inodes += 1;
        self.dirty = true;
        Ok(())
    }

    // ---- logical -> physical block mapping -----------------------------------------

    fn add_blocks(&self, ino: &mut Inode, n: u32) {
        ino.set_blocks(ino.blocks() + n * self.spb);
    }

    /// Map file block `lblk` to a physical block. Returns 0 for a hole when `create` is false.
    fn bmap(&mut self, ino: &mut Inode, goal: u32, lblk: u32, create: bool) -> Result<u32, FsError> {
        let ppb = (self.bs / 4) as u64;
        let l = lblk as u64;
        if l < 12 {
            let mut p = ino.block(l as usize);
            if p == 0 && create {
                p = self.alloc_zeroed_block(goal)?;
                ino.set_block(l as usize, p);
                self.add_blocks(ino, 1);
            }
            return Ok(p);
        }
        let mut rel = l - 12;
        let (slot, depth): (usize, u32) = if rel < ppb {
            (12, 1)
        } else {
            rel -= ppb;
            if rel < ppb * ppb {
                (13, 2)
            } else {
                rel -= ppb * ppb;
                if rel < ppb * ppb * ppb {
                    (14, 3)
                } else {
                    return Err(FsError::TooBig);
                }
            }
        };
        let mut cur = ino.block(slot);
        if cur == 0 {
            if !create {
                return Ok(0);
            }
            cur = self.alloc_zeroed_block(goal)?;
            ino.set_block(slot, cur);
            self.add_blocks(ino, 1);
        }
        let mut d = depth;
        let mut buf = vec![0u8; self.bs];
        loop {
            let span = ppb.pow(d - 1);
            let i = (rel / span) as usize;
            rel %= span;
            self.read_block(cur, &mut buf)?;
            let mut next = rd32(&buf, i * 4);
            if next == 0 {
                if !create {
                    return Ok(0);
                }
                next = self.alloc_zeroed_block(goal)?;
                wr32(&mut buf, i * 4, next);
                self.write_block(cur, &buf)?;
                self.add_blocks(ino, 1);
            }
            if d == 1 {
                return Ok(next);
            }
            cur = next;
            d -= 1;
        }
    }

    /// Free everything under indirect block `blk` at file-relative index >= `keep_from`.
    /// Returns true if `blk` itself became empty and was freed.
    fn free_subtree(&mut self, blk: u32, depth: u32, keep_from: u64, freed: &mut u32) -> Result<bool, FsError> {
        let ppb = (self.bs / 4) as u64;
        let span = ppb.pow(depth - 1);
        let mut buf = vec![0u8; self.bs];
        self.read_block(blk, &mut buf)?;
        let mut changed = false;
        let mut all_zero = true;
        for i in 0..ppb as usize {
            let e = rd32(&buf, i * 4);
            if e == 0 {
                continue;
            }
            let base = i as u64 * span;
            if base + span <= keep_from {
                all_zero = false; // entirely inside the kept range
                continue;
            }
            if depth == 1 {
                self.free_block(e)?;
                *freed += 1;
                wr32(&mut buf, i * 4, 0);
                changed = true;
            } else if self.free_subtree(e, depth - 1, keep_from.saturating_sub(base), freed)? {
                wr32(&mut buf, i * 4, 0);
                changed = true;
            } else {
                all_zero = false;
            }
        }
        if changed {
            self.write_block(blk, &buf)?;
        }
        if all_zero {
            self.free_block(blk)?;
            *freed += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Release all blocks at file-block index >= `keep`.
    fn truncate_blocks(&mut self, ino: &mut Inode, keep: u32) -> Result<(), FsError> {
        if !ino.has_block_pointers() {
            return Ok(());
        }
        let ppb = (self.bs / 4) as u64;
        let mut freed = 0u32;
        for i in (keep as usize)..12 {
            let p = ino.block(i);
            if p != 0 {
                self.free_block(p)?;
                freed += 1;
                ino.set_block(i, 0);
            }
        }
        let bases = [12u64, 12 + ppb, 12 + ppb + ppb * ppb];
        let cover = [ppb, ppb * ppb, ppb * ppb * ppb];
        for k in 0..3 {
            let slot = 12 + k;
            let p = ino.block(slot);
            if p == 0 || (keep as u64) >= bases[k] + cover[k] {
                continue;
            }
            let rel = (keep as u64).saturating_sub(bases[k]);
            if self.free_subtree(p, (k + 1) as u32, rel, &mut freed)? {
                ino.set_block(slot, 0);
            }
        }
        ino.set_blocks(ino.blocks().saturating_sub(freed * self.spb));
        Ok(())
    }

    // ---- directories -------------------------------------------------------------------

    fn check_dir(&mut self, ino: u32) -> Result<Inode, FsError> {
        let i = self.read_inode(ino)?;
        if i.is_dir() { Ok(i) } else { Err(FsError::NotDir) }
    }

    fn dir_find(&mut self, dir: &mut Inode, name: &[u8]) -> Result<Option<(u32, u8)>, FsError> {
        let bs = self.bs;
        let nblocks = dir.size() as usize / bs;
        let mut buf = vec![0u8; bs];
        for lb in 0..nblocks {
            let pb = self.bmap(dir, 0, lb as u32, false)?;
            if pb == 0 {
                continue;
            }
            self.read_block(pb, &mut buf)?;
            let mut off = 0;
            while off < bs {
                let e = parse_dirent(&buf, off, bs)?;
                if e.inode != 0 && e.name_len == name.len() && &buf[off + 8..off + 8 + e.name_len] == name {
                    return Ok(Some((e.inode, e.ftype)));
                }
                off += e.rec_len;
            }
        }
        Ok(None)
    }

    fn dir_add(&mut self, dir_ino: u32, name: &[u8], child: u32, ftype: u8) -> Result<(), FsError> {
        validate_name(name)?;
        let bs = self.bs;
        let need = align4(8 + name.len());
        let ft = if self.filetype { ftype } else { 0 };
        let mut dir = self.read_inode(dir_ino)?;
        let nblocks = dir.size() as usize / bs;
        let mut buf = vec![0u8; bs];
        for lb in 0..nblocks {
            let pb = self.bmap(&mut dir, 0, lb as u32, false)?;
            if pb == 0 {
                continue;
            }
            self.read_block(pb, &mut buf)?;
            let mut off = 0;
            while off < bs {
                let e = parse_dirent(&buf, off, bs)?;
                let used = if e.inode == 0 { 0 } else { align4(8 + e.name_len) };
                if e.rec_len - used >= need {
                    if e.inode == 0 {
                        put_dirent(&mut buf, off, child, e.rec_len, name, ft);
                    } else {
                        wr16(&mut buf, off + 4, used as u16);
                        put_dirent(&mut buf, off + used, child, e.rec_len - used, name, ft);
                    }
                    self.write_block(pb, &buf)?;
                    if dir.flags() & INDEX_FL != 0 {
                        dir.set_flags(dir.flags() & !INDEX_FL);
                        self.write_inode(dir_ino, &dir)?;
                    }
                    return Ok(());
                }
                off += e.rec_len;
            }
        }
        // No room: append a new block.
        let goal = (dir_ino - 1) / self.ipg;
        let pb = self.bmap(&mut dir, goal, nblocks as u32, true)?;
        buf.iter_mut().for_each(|b| *b = 0);
        put_dirent(&mut buf, 0, child, bs, name, ft);
        self.write_block(pb, &buf)?;
        dir.set_size(dir.size() + bs as u32);
        dir.set_flags(dir.flags() & !INDEX_FL);
        self.write_inode(dir_ino, &dir)
    }

    /// Remove the entry called `name`; returns the inode it pointed to.
    fn dir_remove(&mut self, dir_ino: u32, name: &[u8]) -> Result<u32, FsError> {
        let bs = self.bs;
        let mut dir = self.read_inode(dir_ino)?;
        let nblocks = dir.size() as usize / bs;
        let mut buf = vec![0u8; bs];
        for lb in 0..nblocks {
            let pb = self.bmap(&mut dir, 0, lb as u32, false)?;
            if pb == 0 {
                continue;
            }
            self.read_block(pb, &mut buf)?;
            let mut prev: Option<usize> = None;
            let mut off = 0;
            while off < bs {
                let e = parse_dirent(&buf, off, bs)?;
                if e.inode != 0 && e.name_len == name.len() && &buf[off + 8..off + 8 + e.name_len] == name {
                    match prev {
                        Some(p) => {
                            let pl = rd16(&buf, p + 4) as usize;
                            wr16(&mut buf, p + 4, (pl + e.rec_len) as u16);
                        }
                        None => wr32(&mut buf, off, 0),
                    }
                    self.write_block(pb, &buf)?;
                    if dir.flags() & INDEX_FL != 0 {
                        dir.set_flags(dir.flags() & !INDEX_FL);
                        self.write_inode(dir_ino, &dir)?;
                    }
                    return Ok(e.inode);
                }
                prev = Some(off);
                off += e.rec_len;
            }
        }
        Err(FsError::NotFound)
    }

    fn dir_is_empty(&mut self, dir_ino: u32) -> Result<bool, FsError> {
        let bs = self.bs;
        let mut dir = self.read_inode(dir_ino)?;
        let nblocks = dir.size() as usize / bs;
        let mut buf = vec![0u8; bs];
        for lb in 0..nblocks {
            let pb = self.bmap(&mut dir, 0, lb as u32, false)?;
            if pb == 0 {
                continue;
            }
            self.read_block(pb, &mut buf)?;
            let mut off = 0;
            while off < bs {
                let e = parse_dirent(&buf, off, bs)?;
                if e.inode != 0 {
                    let n = &buf[off + 8..off + 8 + e.name_len];
                    if n != b"." && n != b".." {
                        return Ok(false);
                    }
                }
                off += e.rec_len;
            }
        }
        Ok(true)
    }

    /// Point a directory's ".." entry at `new_parent`.
    fn dir_set_parent(&mut self, dir_ino: u32, new_parent: u32) -> Result<(), FsError> {
        let mut dir = self.read_inode(dir_ino)?;
        let pb = self.bmap(&mut dir, 0, 0, false)?;
        if pb == 0 {
            return Err(FsError::Corrupt);
        }
        let mut buf = vec![0u8; self.bs];
        self.read_block(pb, &mut buf)?;
        let dot = parse_dirent(&buf, 0, self.bs)?;
        let dotdot = parse_dirent(&buf, dot.rec_len, self.bs)?;
        let _ = dotdot.ftype;
        wr32(&mut buf, dot.rec_len, new_parent);
        self.write_block(pb, &buf)
    }

    // ---- inode lifetime helpers ------------------------------------------------------------

    fn adj_links(&mut self, ino: u32, delta: i32) -> Result<(), FsError> {
        let mut i = self.read_inode(ino)?;
        let n = (i.links() as i32 + delta).clamp(0, 65535) as u16;
        i.set_links(n);
        i.set_ctime((self.clock)());
        self.write_inode(ino, &i)
    }

    /// Free an inode's data and the inode itself (link count already accounted for).
    fn destroy_inode(&mut self, ino: u32) -> Result<(), FsError> {
        let mut i = self.read_inode(ino)?;
        let was_dir = i.is_dir();
        self.truncate_blocks(&mut i, 0)?;
        i.set_links(0);
        i.set_dtime((self.clock)());
        if i.has_block_pointers() {
            i.set_size(0);
            i.set_blocks(0);
        }
        self.write_inode(ino, &i)?;
        self.free_inode(ino, was_dir)
    }

    fn touch_dir(&mut self, ino: u32) -> Result<(), FsError> {
        let mut d = self.read_inode(ino)?;
        let now = (self.clock)();
        d.set_mtime(now);
        d.set_ctime(now);
        self.write_inode(ino, &d)
    }

    fn new_node(&mut self, dir: u32, name: &str, mode: u16, ftype: u8) -> Result<u32, FsError> {
        let nb = name.as_bytes();
        validate_name(nb)?;
        let mut d = self.check_dir(dir)?;
        if d.links() == 0 {
            return Err(FsError::NotFound);
        }
        if self.dir_find(&mut d, nb)?.is_some() {
            return Err(FsError::Exists);
        }
        let ino = self.alloc_inode((dir - 1) / self.ipg, false)?;
        let now = (self.clock)();
        let mut i = Inode::zeroed();
        i.set_mode(mode);
        i.set_links(1);
        i.set_atime(now);
        i.set_ctime(now);
        i.set_mtime(now);
        if let Err(e) = self.write_inode(ino, &i).and_then(|_| self.dir_add(dir, nb, ino, ftype)) {
            let _ = self.free_inode(ino, false);
            return Err(e);
        }
        self.touch_dir(dir)?;
        self.sync()?;
        Ok(ino)
    }

    // ================================================================================
    //                                 Public API
    // ================================================================================

    pub fn root(&self) -> u32 {
        ROOT_INO
    }

    pub fn stat(&mut self, ino: u32) -> Result<Stat, FsError> {
        let i = self.read_inode(ino)?;
        Ok(Stat {
            ino,
            mode: i.mode(),
            file_type: FileType::from_mode(i.mode()),
            uid: i.uid(),
            gid: i.gid(),
            size: i.size() as u64,
            links: i.links(),
            blocks: i.blocks(),
            atime: i.atime(),
            mtime: i.mtime(),
            ctime: i.ctime(),
        })
    }

    pub fn lookup(&mut self, dir: u32, name: &str) -> Result<u32, FsError> {
        let nb = name.as_bytes();
        if nb.len() > MAX_NAME {
            return Err(FsError::NameTooLong);
        }
        let mut d = self.check_dir(dir)?;
        self.dir_find(&mut d, nb)?.map(|(i, _)| i).ok_or(FsError::NotFound)
    }

    pub fn readdir(&mut self, dir: u32) -> Result<Vec<DirEntry>, FsError> {
        let bs = self.bs;
        let mut d = self.check_dir(dir)?;
        let nblocks = d.size() as usize / bs;
        let mut out = Vec::new();
        let mut buf = vec![0u8; bs];
        for lb in 0..nblocks {
            let pb = self.bmap(&mut d, 0, lb as u32, false)?;
            if pb == 0 {
                continue;
            }
            self.read_block(pb, &mut buf)?;
            let mut off = 0;
            while off < bs {
                let e = parse_dirent(&buf, off, bs)?;
                if e.inode != 0 {
                    out.push(DirEntry {
                        ino: e.inode,
                        name: String::from_utf8_lossy(&buf[off + 8..off + 8 + e.name_len]).into_owned(),
                        file_type: if self.filetype { FileType::from_dirent(e.ftype) } else { FileType::Unknown },
                    });
                }
                off += e.rec_len;
            }
        }
        Ok(out)
    }

    pub fn read(&mut self, ino: u32, off: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut i = self.read_inode(ino)?;
        if i.is_dir() {
            return Err(FsError::IsDir);
        }
        if !i.is_reg() {
            return Err(FsError::Invalid);
        }
        let size = i.size() as u64;
        if off >= size {
            return Ok(0);
        }
        let n = min(buf.len() as u64, size - off) as usize;
        let bs = self.bs;
        let mut tmp = vec![0u8; bs];
        let mut done = 0usize;
        while done < n {
            let pos = off + done as u64;
            let lb = (pos / bs as u64) as u32;
            let bo = (pos % bs as u64) as usize;
            let chunk = min(bs - bo, n - done);
            let pb = self.bmap(&mut i, 0, lb, false)?;
            if pb == 0 {
                buf[done..done + chunk].iter_mut().for_each(|b| *b = 0); // sparse hole
            } else {
                self.read_block(pb, &mut tmp)?;
                buf[done..done + chunk].copy_from_slice(&tmp[bo..bo + chunk]);
            }
            done += chunk;
        }
        Ok(n)
    }

    pub fn write(&mut self, ino: u32, off: u64, data: &[u8]) -> Result<usize, FsError> {
        let mut i = self.read_inode(ino)?;
        if i.is_dir() {
            return Err(FsError::IsDir);
        }
        if !i.is_reg() {
            return Err(FsError::Invalid);
        }
        if data.is_empty() {
            return Ok(0);
        }
        if off >= MAX_FILE_SIZE {
            return Err(FsError::TooBig);
        }
        let mut len = data.len();
        if off + len as u64 > MAX_FILE_SIZE {
            len = (MAX_FILE_SIZE - off) as usize;
        }
        let goal = (ino - 1) / self.ipg;
        let bs = self.bs;
        let mut buf = vec![0u8; bs];
        let mut done = 0usize;
        let mut failure: Option<FsError> = None;
        while done < len {
            let pos = off + done as u64;
            let lb = (pos / bs as u64) as u32;
            let bo = (pos % bs as u64) as usize;
            let n = min(bs - bo, len - done);
            let pb = match self.bmap(&mut i, goal, lb, true) {
                Ok(p) => p,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            };
            if n == bs {
                buf.copy_from_slice(&data[done..done + n]);
            } else {
                self.read_block(pb, &mut buf)?;
                buf[bo..bo + n].copy_from_slice(&data[done..done + n]);
            }
            self.write_block(pb, &buf)?;
            done += n;
        }
        if done > 0 {
            let end = off + done as u64;
            if end > i.size() as u64 {
                i.set_size(end as u32);
            }
            let now = (self.clock)();
            i.set_mtime(now);
            i.set_ctime(now);
        }
        self.write_inode(ino, &i)?; // persists newly allocated blocks even on partial failure
        self.sync()?;
        match failure {
            Some(e) if done == 0 => Err(e),
            _ => Ok(done),
        }
    }

    pub fn truncate(&mut self, ino: u32, size: u64) -> Result<(), FsError> {
        let mut i = self.read_inode(ino)?;
        if i.is_dir() {
            return Err(FsError::IsDir);
        }
        if !i.is_reg() {
            return Err(FsError::Invalid);
        }
        if size > MAX_FILE_SIZE {
            return Err(FsError::TooBig);
        }
        let bs = self.bs as u64;
        if size < i.size() as u64 {
            let keep = ((size + bs - 1) / bs) as u32;
            self.truncate_blocks(&mut i, keep)?;
            if size % bs != 0 {
                // zero the tail of the last kept block so re-extension reads zeros
                let pb = self.bmap(&mut i, 0, (size / bs) as u32, false)?;
                if pb != 0 {
                    let mut buf = vec![0u8; self.bs];
                    self.read_block(pb, &mut buf)?;
                    buf[(size % bs) as usize..].iter_mut().for_each(|b| *b = 0);
                    self.write_block(pb, &buf)?;
                }
            }
        }
        i.set_size(size as u32);
        let now = (self.clock)();
        i.set_mtime(now);
        i.set_ctime(now);
        self.write_inode(ino, &i)?;
        self.sync()
    }

    pub fn create(&mut self, dir: u32, name: &str, mode: u16) -> Result<u32, FsError> {
        self.new_node(dir, name, S_IFREG | (mode & 0o7777), FT_REG)
    }

    pub fn mkdir(&mut self, dir: u32, name: &str, mode: u16) -> Result<u32, FsError> {
        let nb = name.as_bytes();
        validate_name(nb)?;
        let mut d = self.check_dir(dir)?;
        if d.links() == 0 {
            return Err(FsError::NotFound);
        }
        if d.links() >= MAX_LINKS {
            return Err(FsError::TooManyLinks);
        }
        if self.dir_find(&mut d, nb)?.is_some() {
            return Err(FsError::Exists);
        }
        let goal = (dir - 1) / self.ipg;
        let ino = self.alloc_inode(goal, true)?;
        let blk = match self.alloc_zeroed_block(goal) {
            Ok(b) => b,
            Err(e) => {
                let _ = self.free_inode(ino, true);
                return Err(e);
            }
        };
        let bs = self.bs;
        let ft = if self.filetype { FT_DIR } else { 0 };
        let mut buf = vec![0u8; bs];
        put_dirent(&mut buf, 0, ino, 12, b".", ft);
        put_dirent(&mut buf, 12, dir, bs - 12, b"..", ft);
        let now = (self.clock)();
        let mut i = Inode::zeroed();
        i.set_mode(S_IFDIR | (mode & 0o7777));
        i.set_links(2);
        i.set_size(bs as u32);
        i.set_blocks(self.spb);
        i.set_block(0, blk);
        i.set_atime(now);
        i.set_ctime(now);
        i.set_mtime(now);
        let res = self
            .write_block(blk, &buf)
            .and_then(|_| self.write_inode(ino, &i))
            .and_then(|_| self.dir_add(dir, nb, ino, FT_DIR));
        if let Err(e) = res {
            let _ = self.free_block(blk);
            let _ = self.free_inode(ino, true);
            return Err(e);
        }
        self.adj_links(dir, 1)?;
        self.touch_dir(dir)?;
        self.sync()?;
        Ok(ino)
    }

    pub fn symlink(&mut self, dir: u32, name: &str, target: &str) -> Result<u32, FsError> {
        let tb = target.as_bytes();
        if tb.is_empty() {
            return Err(FsError::Invalid);
        }
        if tb.len() >= self.bs {
            return Err(FsError::NameTooLong);
        }
        let nb = name.as_bytes();
        validate_name(nb)?;
        let mut d = self.check_dir(dir)?;
        if d.links() == 0 {
            return Err(FsError::NotFound);
        }
        if self.dir_find(&mut d, nb)?.is_some() {
            return Err(FsError::Exists);
        }
        let goal = (dir - 1) / self.ipg;
        let ino = self.alloc_inode(goal, false)?;
        let now = (self.clock)();
        let mut i = Inode::zeroed();
        i.set_mode(S_IFLNK | 0o777);
        i.set_links(1);
        i.set_size(tb.len() as u32);
        i.set_atime(now);
        i.set_ctime(now);
        i.set_mtime(now);
        let mut data_blk = 0u32;
        if tb.len() < FAST_SYMLINK_MAX {
            i.raw[40..40 + tb.len()].copy_from_slice(tb); // "fast" symlink: text lives in i_block[]
        } else {
            data_blk = match self.alloc_zeroed_block(goal) {
                Ok(b) => b,
                Err(e) => {
                    let _ = self.free_inode(ino, false);
                    return Err(e);
                }
            };
            let mut buf = vec![0u8; self.bs];
            buf[..tb.len()].copy_from_slice(tb);
            if let Err(e) = self.write_block(data_blk, &buf) {
                let _ = self.free_block(data_blk);
                let _ = self.free_inode(ino, false);
                return Err(e);
            }
            i.set_block(0, data_blk);
            i.set_blocks(self.spb);
        }
        let res = self.write_inode(ino, &i).and_then(|_| self.dir_add(dir, nb, ino, FT_SYMLINK));
        if let Err(e) = res {
            if data_blk != 0 {
                let _ = self.free_block(data_blk);
            }
            let _ = self.free_inode(ino, false);
            return Err(e);
        }
        self.touch_dir(dir)?;
        self.sync()?;
        Ok(ino)
    }

    pub fn readlink(&mut self, ino: u32) -> Result<Vec<u8>, FsError> {
        let mut i = self.read_inode(ino)?;
        if !i.is_lnk() {
            return Err(FsError::Invalid);
        }
        let len = i.size() as usize;
        if i.is_fast_symlink() {
            return Ok(i.raw[40..40 + len].to_vec());
        }
        let pb = self.bmap(&mut i, 0, 0, false)?;
        if pb == 0 || len > self.bs {
            return Err(FsError::Corrupt);
        }
        let mut buf = vec![0u8; self.bs];
        self.read_block(pb, &mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Hard link: make `name` in `dir` refer to existing inode `target`.
    pub fn link(&mut self, dir: u32, name: &str, target: u32) -> Result<(), FsError> {
        let nb = name.as_bytes();
        validate_name(nb)?;
        let t = self.read_inode(target)?;
        if t.is_dir() {
            return Err(FsError::IsDir);
        }
        if t.links() >= MAX_LINKS {
            return Err(FsError::TooManyLinks);
        }
        let mut d = self.check_dir(dir)?;
        if self.dir_find(&mut d, nb)?.is_some() {
            return Err(FsError::Exists);
        }
        let ft = match t.kind() {
            S_IFREG => FT_REG,
            S_IFLNK => FT_SYMLINK,
            0x2000 => 3,
            0x6000 => 4,
            0x1000 => 5,
            0xC000 => 6,
            _ => 0,
        };
        self.dir_add(dir, nb, target, ft)?;
        self.adj_links(target, 1)?;
        self.touch_dir(dir)?;
        self.sync()
    }

    pub fn unlink(&mut self, dir: u32, name: &str) -> Result<(), FsError> {
        let nb = name.as_bytes();
        validate_name(nb)?;
        let mut d = self.check_dir(dir)?;
        let (ino, _) = self.dir_find(&mut d, nb)?.ok_or(FsError::NotFound)?;
        let i = self.read_inode(ino)?;
        if i.is_dir() {
            return Err(FsError::IsDir);
        }
        self.dir_remove(dir, nb)?;
        if i.links() <= 1 {
            self.destroy_inode(ino)?;
        } else {
            self.adj_links(ino, -1)?;
        }
        self.touch_dir(dir)?;
        self.sync()
    }

    pub fn rmdir(&mut self, dir: u32, name: &str) -> Result<(), FsError> {
        let nb = name.as_bytes();
        validate_name(nb)?;
        if nb == b"." || nb == b".." {
            return Err(FsError::Invalid);
        }
        let mut d = self.check_dir(dir)?;
        let (ino, _) = self.dir_find(&mut d, nb)?.ok_or(FsError::NotFound)?;
        let i = self.read_inode(ino)?;
        if !i.is_dir() {
            return Err(FsError::NotDir);
        }
        if ino == ROOT_INO || !self.dir_is_empty(ino)? {
            return Err(FsError::NotEmpty);
        }
        self.dir_remove(dir, nb)?;
        self.destroy_inode(ino)?;
        self.adj_links(dir, -1)?; // the child's ".." no longer points here
        self.touch_dir(dir)?;
        self.sync()
    }

    pub fn rename(&mut self, old_dir: u32, old_name: &str, new_dir: u32, new_name: &str) -> Result<(), FsError> {
        let (on, nn) = (old_name.as_bytes(), new_name.as_bytes());
        validate_name(on)?;
        validate_name(nn)?;
        if on == b"." || on == b".." || nn == b"." || nn == b".." {
            return Err(FsError::Invalid);
        }
        if old_dir == new_dir && on == nn {
            return Ok(());
        }
        let mut od = self.check_dir(old_dir)?;
        let mut nd = self.check_dir(new_dir)?;
        let (sino, sft) = self.dir_find(&mut od, on)?.ok_or(FsError::NotFound)?;
        let src = self.read_inode(sino)?;
        let is_dir = src.is_dir();

        if is_dir {
            // refuse to move a directory into its own subtree
            let mut cur = new_dir;
            loop {
                if cur == sino {
                    return Err(FsError::Invalid);
                }
                if cur == ROOT_INO {
                    break;
                }
                let p = self.lookup(cur, "..")?;
                if p == cur {
                    break;
                }
                cur = p;
            }
        }

        if let Some((eino, _)) = self.dir_find(&mut nd, nn)? {
            if eino == sino {
                // both names are hard links to the same inode: just drop the old name
                self.dir_remove(old_dir, on)?;
                self.adj_links(sino, -1)?;
                return self.sync();
            }
            let ex = self.read_inode(eino)?;
            if ex.is_dir() {
                if !is_dir {
                    return Err(FsError::IsDir);
                }
                if !self.dir_is_empty(eino)? {
                    return Err(FsError::NotEmpty);
                }
            } else if is_dir {
                return Err(FsError::NotDir);
            }
            self.dir_remove(new_dir, nn)?;
            if ex.is_dir() {
                self.destroy_inode(eino)?;
                self.adj_links(new_dir, -1)?;
            } else if ex.links() <= 1 {
                self.destroy_inode(eino)?;
            } else {
                self.adj_links(eino, -1)?;
            }
        }

        self.dir_add(new_dir, nn, sino, sft)?;
        self.dir_remove(old_dir, on)?;
        if is_dir && old_dir != new_dir {
            self.dir_set_parent(sino, new_dir)?;
            self.adj_links(old_dir, -1)?;
            self.adj_links(new_dir, 1)?;
        }
        self.adj_links(sino, 0)?; // bump ctime
        self.touch_dir(old_dir)?;
        self.touch_dir(new_dir)?;
        self.sync()
    }

    pub fn chmod(&mut self, ino: u32, mode: u16) -> Result<(), FsError> {
        let mut i = self.read_inode(ino)?;
        i.set_mode((i.mode() & S_IFMT) | (mode & 0o7777));
        i.set_ctime((self.clock)());
        self.write_inode(ino, &i)?;
        self.sync()
    }

    pub fn chown(&mut self, ino: u32, uid: u32, gid: u32) -> Result<(), FsError> {
        let mut i = self.read_inode(ino)?;
        wr16(&mut i.raw, 2, uid as u16);
        wr16(&mut i.raw, 120, (uid >> 16) as u16);
        wr16(&mut i.raw, 24, gid as u16);
        wr16(&mut i.raw, 122, (gid >> 16) as u16);
        i.set_ctime((self.clock)());
        self.write_inode(ino, &i)?;
        self.sync()
    }

    // ---- path helpers ------------------------------------------------------------------------

    /// Resolve `path` (absolute, or relative to `cwd`). Symlinks in intermediate
    /// components are always followed; the last one only if `follow_last`
    /// (i.e. `stat` vs `lstat`). Loop limit: 40 links, like Linux.
    pub fn resolve(&mut self, cwd: u32, path: &str, follow_last: bool) -> Result<u32, FsError> {
        let mut depth = 0;
        self.walk(cwd, path, follow_last, &mut depth)
    }

    fn walk(&mut self, start: u32, path: &str, follow_last: bool, depth: &mut u32) -> Result<u32, FsError> {
        let mut cur = if path.starts_with('/') { ROOT_INO } else { start };
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        for (idx, c) in comps.iter().enumerate() {
            let last = idx + 1 == comps.len();
            let next = self.lookup(cur, c)?;
            let ni = self.read_inode(next)?;
            if ni.is_lnk() && (!last || follow_last) {
                *depth += 1;
                if *depth > MAX_SYMLOOP {
                    return Err(FsError::Loop);
                }
                let raw = self.readlink(next)?;
                let target = core::str::from_utf8(&raw).map_err(|_| FsError::Invalid)?;
                cur = self.walk(cur, target, true, depth)?; // relative targets resolve against the link's directory
            } else {
                cur = next;
            }
        }
        Ok(cur)
    }

    /// Split `path` into (parent directory inode, final name). Parent symlinks are followed.
    pub fn resolve_parent<'a>(&mut self, cwd: u32, path: &'a str) -> Result<(u32, &'a str), FsError> {
        let t = path.trim_end_matches('/');
        if t.is_empty() {
            return Err(FsError::Invalid);
        }
        let (dirpart, name) = match t.rfind('/') {
            Some(0) => ("/", &t[1..]),
            Some(p) => (&t[..p], &t[p + 1..]),
            None => (".", t),
        };
        Ok((self.resolve(cwd, dirpart, true)?, name))
    }

    /// Convenience for shell commands: read a whole file.
    pub fn read_to_vec(&mut self, ino: u32) -> Result<Vec<u8>, FsError> {
        let size = self.stat(ino)?.size as usize;
        let mut v = vec![0u8; size];
        let n = self.read(ino, 0, &mut v)?;
        v.truncate(n);
        Ok(v)
    }

    /// Convenience for shell commands: create-or-truncate `path` and write `data`.
    pub fn write_file(&mut self, cwd: u32, path: &str, data: &[u8]) -> Result<u32, FsError> {
        let (dir, name) = self.resolve_parent(cwd, path)?;
        let ino = match self.lookup(dir, name) {
            Ok(i) => {
                self.truncate(i, 0)?;
                i
            }
            Err(FsError::NotFound) => self.create(dir, name, 0o644)?,
            Err(e) => return Err(e),
        };
        let mut off = 0;
        while off < data.len() {
            let n = self.write(ino, off as u64, &data[off..])?;
            if n == 0 {
                return Err(FsError::NoSpace);
            }
            off += n;
        }
        Ok(ino)
    }
}
