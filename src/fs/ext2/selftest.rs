//! End-to-end self test. Works on any `BlockDevice` (RamDisk or the real ATA disk).
//! WARNING: it formats the device it is given.

use super::*;
use core::fmt::Arguments;

#[derive(Debug)]
pub enum TestError {
    Fs(FsError),
    Check(&'static str),
}
impl From<FsError> for TestError {
    fn from(e: FsError) -> Self {
        TestError::Fs(e)
    }
}

fn ensure(c: bool, msg: &'static str) -> Result<(), TestError> {
    if c { Ok(()) } else { Err(TestError::Check(msg)) }
}

fn pattern(i: usize) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 8)) as u8
}

/// `big_kib`: size of the large test file. Use >= 5120 with 4K blocks (reaches
/// double-indirect blocks), >= 512 with 1K blocks.
pub fn run<D: BlockDevice>(
    dev: D,
    opts: &FormatOptions,
    big_kib: usize,
    log: &mut dyn FnMut(Arguments<'_>),
) -> Result<D, TestError> {
    log(format_args!("[ext2] formatting..."));
    let mut fs = Ext2::format(dev, opts, zero_clock)?;
    let u0 = fs.usage();
    log(format_args!("[ext2] {} blocks x {} B, {} inodes, {} blocks free", u0.total_blocks, u0.block_size, u0.total_inodes, u0.free_blocks));

    let root = fs.root();
    let names: Vec<String> = fs.readdir(root)?.into_iter().map(|e| e.name).collect();
    ensure(names.iter().any(|n| n == "lost+found"), "lost+found missing")?;

    // directories, small file
    let etc = fs.mkdir(root, "etc", 0o755)?;
    let hello = fs.create(etc, "hello.txt", 0o644)?;
    fs.write(hello, 0, b"hello oxenna\n")?;
    ensure(fs.read_to_vec(hello)? == b"hello oxenna\n", "small file readback")?;
    log(format_args!("[ext2] small file ok"));

    // large file: exercises direct, indirect and (depending on sizes) double-indirect blocks
    let big = fs.create(root, "big.bin", 0o644)?;
    let total = big_kib * 1024;
    let mut off = 0;
    while off < total {
        let n = (total - off).min(32 * 1024);
        let chunk: Vec<u8> = (off..off + n).map(pattern).collect();
        ensure(fs.write(big, off as u64, &chunk)? == n, "short write")?;
        off += n;
    }
    let back = fs.read_to_vec(big)?;
    ensure(back.len() == total && back.iter().enumerate().all(|(i, &b)| b == pattern(i)), "big file readback")?;
    log(format_args!("[ext2] {} KiB file ok", big_kib));

    // truncate must give blocks back
    let before = fs.usage().free_blocks;
    fs.truncate(big, 10_000)?;
    ensure(fs.usage().free_blocks > before, "truncate did not free blocks")?;
    ensure(fs.stat(big)?.size == 10_000, "truncate size")?;
    let back = fs.read_to_vec(big)?;
    ensure(back.iter().enumerate().all(|(i, &b)| b == pattern(i)), "data after truncate")?;
    log(format_args!("[ext2] truncate ok"));

    // symlinks: relative (fast), absolute (fast), long (slow, data block)
    fs.symlink(etc, "rel.lnk", "hello.txt")?;
    fs.symlink(root, "abs.lnk", "/etc/hello.txt")?;
    let long_target = "/etc/../etc/./../etc/./hello.txt";
    let long_pad: String = core::iter::repeat("./").take(20).collect::<String>() + long_target;
    ensure(long_pad.len() > FAST_SYMLINK_MAX, "long symlink not long")?;
    fs.symlink(root, "long.lnk", &long_pad)?;
    for p in ["/etc/rel.lnk", "/abs.lnk", "/long.lnk"] {
        let i = fs.resolve(root, p, true)?;
        ensure(i == hello, "symlink resolve")?;
    }
    let l = fs.resolve(root, "/abs.lnk", false)?;
    ensure(fs.stat(l)?.file_type == FileType::Symlink, "lstat type")?;
    ensure(fs.readlink(l)? == b"/etc/hello.txt", "readlink")?;
    fs.symlink(root, "loop1", "loop2")?;
    fs.symlink(root, "loop2", "loop1")?;
    ensure(fs.resolve(root, "/loop1", true) == Err(FsError::Loop), "loop detection")?;
    log(format_args!("[ext2] symlinks ok"));

    // hard link + rename + unlink + rmdir
    fs.link(root, "hard.txt", hello)?;
    ensure(fs.stat(hello)?.links == 2, "link count")?;
    fs.rename(etc, "hello.txt", root, "hello2.txt")?;
    ensure(fs.lookup(etc, "hello.txt") == Err(FsError::NotFound), "old name gone")?;
    ensure(fs.resolve(root, "/abs.lnk", true) == Err(FsError::NotFound), "dangling symlink")?;
    fs.unlink(root, "hard.txt")?;
    ensure(fs.stat(hello)?.links == 1, "link count after unlink")?;
    ensure(fs.rmdir(root, "etc") == Err(FsError::NotEmpty), "rmdir non-empty")?;
    fs.unlink(etc, "rel.lnk")?;
    fs.rmdir(root, "etc")?;
    ensure(fs.stat(root)?.links == 3, "root link count")?; // ".", "..", lost+found's ".."
    log(format_args!("[ext2] link/rename/unlink/rmdir ok"));

    // many entries force directory growth
    let d = fs.mkdir(root, "many", 0o755)?;
    for i in 0..300 {
        let n = alloc::format!("file_with_a_reasonably_long_name_{i}");
        fs.create(d, &n, 0o600)?;
    }
    ensure(fs.readdir(d)?.len() == 302, "dir growth")?;
    log(format_args!("[ext2] directory growth ok"));

    // persistence across remount
    let dev = fs.into_device()?;
    let mut fs = Ext2::mount(dev)?;
    let root = fs.root();
    let h = fs.resolve(root, "/hello2.txt", true)?;
    ensure(fs.read_to_vec(h)? == b"hello oxenna\n", "remount: small file")?;
    let b = fs.resolve(root, "/big.bin", true)?;
    ensure(fs.stat(b)?.size == 10_000, "remount: big file")?;
    let m = fs.resolve(root, "/many", true)?;
    ensure(fs.readdir(m)?.len() == 302, "remount: dir")?;
    log(format_args!("[ext2] remount ok -- ALL TESTS PASSED"));
    Ok(fs.into_device()?)
}

/// Kernel-test friendly entry point on a 32 MiB RAM disk (no QEMU disk required).
pub fn run_on_ramdisk(log: &mut dyn FnMut(Arguments<'_>)) -> Result<(), TestError> {
    use crate::drivers::block::RamDisk;
    let opts = FormatOptions { block_size: 1024, bytes_per_inode: 8192, label: "ramtest" };
    run(RamDisk::new(64 * 1024), &opts, 600, log).map(|_| ())
}
