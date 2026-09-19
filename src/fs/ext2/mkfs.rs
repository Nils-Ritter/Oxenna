//! Formatter: writes a fresh, e2fsck-clean ext2 (rev 1, FILETYPE + SPARSE_SUPER).

use super::*;

pub struct FormatOptions<'a> {
    /// 1024, 2048 or 4096.
    pub block_size: usize,
    /// One inode per this many bytes of disk (mke2fs default is 16384).
    pub bytes_per_inode: u32,
    /// Volume label, at most 16 bytes.
    pub label: &'a str,
}

impl Default for FormatOptions<'_> {
    fn default() -> Self {
        FormatOptions { block_size: 4096, bytes_per_inode: 16384, label: "" }
    }
}

impl<D: BlockDevice> Ext2<D> {
    /// Destroys everything on `dev`, writes a new filesystem and returns it mounted.
    pub fn format(mut dev: D, opts: &FormatOptions, clock: fn() -> u32) -> Result<Self, FsError> {
        let bs = opts.block_size;
        if !(bs == 1024 || bs == 2048 || bs == 4096) {
            return Err(FsError::Invalid);
        }
        let spb = (bs / SECTOR_SIZE) as u64;
        let now = clock();
        let total = (dev.sector_count() / spb).min(0xFFFF_0000) as u32;
        let first_data: u32 = if bs == 1024 { 1 } else { 0 };
        if total < first_data + 64 {
            return Err(FsError::Invalid); // disk too small
        }
        let bpg = (bs * 8) as u32;
        let ipb = (bs / 128) as u32; // inodes per block

        // ---- geometry ----------------------------------------------------------
        let mut usable = total - first_data;
        let (mut ngroups, mut ipg, mut itb, mut gdt_blocks) = (0u32, 0u32, 0u32, 0u32);
        loop {
            let ng = (usable + bpg - 1) / bpg;
            let want = ((usable as u64 * bs as u64) / opts.bytes_per_inode.max(1024) as u64).max(32) as u32;
            let mut ip = (want + ng - 1) / ng;
            ip = ((ip + ipb - 1) / ipb) * ipb;
            ip = ip.max(((16 + ipb - 1) / ipb) * ipb).min(bpg);
            let it = ip / ipb;
            let gb = (ng * 32 + bs as u32 - 1) / bs as u32;
            let last_blocks = usable - (ng - 1) * bpg;
            let overhead = 2 + it + if has_super(ng - 1) { 1 + gb } else { 0 };
            ngroups = ng;
            ipg = ip;
            itb = it;
            gdt_blocks = gb;
            if ng == 1 || last_blocks >= overhead + 8 {
                break;
            }
            usable = (ng - 1) * bpg; // trailing group too small to be useful: drop it
        }
        {
            let overhead0 = 1 + gdt_blocks + 2 + itb;
            let g0_blocks = min(usable, bpg);
            if g0_blocks < overhead0 + 16 {
                return Err(FsError::Invalid);
            }
        }
        let blocks_count = first_data + usable;
        let inodes_count = ngroups * ipg;
        let first_ino = 11u32;

        // ---- per-group metadata ---------------------------------------------------
        let mut gdt = vec![0u8; gdt_blocks as usize * bs];
        let mut total_free_blocks = 0u32;
        let mut total_free_inodes = 0u32;
        let zero_chunk = vec![0u8; bs * 16];

        for g in 0..ngroups {
            let gstart = first_data + g * bpg;
            let gblocks = if g == ngroups - 1 { usable - g * bpg } else { bpg };
            let mut off = gstart;
            if has_super(g) {
                off += 1 + gdt_blocks; // superblock (copy) + descriptor table
            }
            let (bb, ib, it) = (off, off + 1, off + 2);
            let data_start = it + itb;
            let meta = (data_start - gstart) as usize;

            let mut bm = vec![0u8; bs];
            for bit in 0..meta {
                bit_set(&mut bm, bit);
            }
            for bit in gblocks as usize..bs * 8 {
                bit_set(&mut bm, bit); // padding past end of group
            }
            let mut im = vec![0u8; bs];
            let reserved = if g == 0 { first_ino - 1 } else { 0 };
            for bit in 0..reserved as usize {
                bit_set(&mut im, bit);
            }
            for bit in ipg as usize..bs * 8 {
                bit_set(&mut im, bit);
            }
            dev.write_sectors(bb as u64 * spb, &bm)?;
            dev.write_sectors(ib as u64 * spb, &im)?;

            // zero the inode table
            let mut left = itb as usize;
            let mut blk = it as u64;
            while left > 0 {
                let n = left.min(16);
                dev.write_sectors(blk * spb, &zero_chunk[..n * bs])?;
                blk += n as u64;
                left -= n;
            }

            let free_b = gblocks - meta as u32;
            let free_i = ipg - reserved;
            let o = Self::gd_off(g);
            wr32(&mut gdt, o, bb);
            wr32(&mut gdt, o + 4, ib);
            wr32(&mut gdt, o + 8, it);
            wr16(&mut gdt, o + 12, free_b as u16);
            wr16(&mut gdt, o + 14, free_i as u16);
            total_free_blocks += free_b;
            total_free_inodes += free_i;
        }

        // ---- superblock -------------------------------------------------------------------
        let mut sb = [0u8; 1024];
        let log = (bs / 1024).trailing_zeros();
        wr32(&mut sb, 0, inodes_count);
        wr32(&mut sb, 4, blocks_count);
        wr32(&mut sb, 12, total_free_blocks);
        wr32(&mut sb, 16, total_free_inodes);
        wr32(&mut sb, 20, first_data);
        wr32(&mut sb, 24, log);
        wr32(&mut sb, 28, log);
        wr32(&mut sb, 32, bpg);
        wr32(&mut sb, 36, bpg);
        wr32(&mut sb, 40, ipg);
        wr32(&mut sb, 48, now);
        wr16(&mut sb, 54, 0xFFFF); // max mount count: -1 (never force fsck)
        wr16(&mut sb, 56, MAGIC);
        wr16(&mut sb, 58, 1); // state: valid
        wr16(&mut sb, 60, 1); // errors: continue
        wr32(&mut sb, 64, now);
        wr32(&mut sb, 76, 1); // rev 1 (dynamic)
        wr32(&mut sb, 84, first_ino);
        wr16(&mut sb, 88, 128);
        wr32(&mut sb, 96, INCOMPAT_FILETYPE);
        wr32(&mut sb, 100, RO_COMPAT_SPARSE_SUPER);
        let mut x = ((now as u64) << 32) ^ (total as u64) ^ 0x9E37_79B9_7F4A_7C15;
        for i in 0..2 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            sb[104 + i * 8..112 + i * 8].copy_from_slice(&x.to_le_bytes());
        }
        let label = opts.label.as_bytes();
        let ln = label.len().min(16);
        sb[120..120 + ln].copy_from_slice(&label[..ln]);

        dev.write_sectors(2, &sb)?;
        dev.write_sectors((first_data as u64 + 1) * spb, &gdt)?;
        // backup superblocks + descriptor tables
        for g in 1..ngroups {
            if !has_super(g) {
                continue;
            }
            let gstart = first_data + g * bpg;
            let mut copy = vec![0u8; bs];
            copy[..1024].copy_from_slice(&sb);
            wr16(&mut copy, 90, g as u16); // s_block_group_nr
            dev.write_sectors(gstart as u64 * spb, &copy)?;
            dev.write_sectors((gstart as u64 + 1) * spb, &gdt)?;
        }
        dev.flush()?;

        // ---- root directory and lost+found via the normal code paths -------------------------
        let mut fs = Ext2::mount(dev)?;
        fs.set_clock(clock);
        let blk = fs.alloc_zeroed_block(0)?;
        let ft = FT_DIR;
        let mut buf = vec![0u8; bs];
        put_dirent(&mut buf, 0, ROOT_INO, 12, b".", ft);
        put_dirent(&mut buf, 12, ROOT_INO, bs - 12, b"..", ft);
        fs.write_block(blk, &buf)?;
        let mut root = Inode::zeroed();
        root.set_mode(S_IFDIR | 0o755);
        root.set_links(2); // mkdir(lost+found) adds the third
        root.set_size(bs as u32);
        root.set_blocks(fs.spb);
        root.set_block(0, blk);
        root.set_atime(now);
        root.set_ctime(now);
        root.set_mtime(now);
        fs.write_inode(ROOT_INO, &root)?;
        let d = fs.gd_used_dirs(0);
        fs.gd_set_used_dirs(0, d + 1);
        fs.dirty = true;
        let lf = fs.mkdir(ROOT_INO, "lost+found", 0o700)?;
        debug_assert_eq!(lf, first_ino);
        fs.sync()?;
        Ok(fs)
    }
}
