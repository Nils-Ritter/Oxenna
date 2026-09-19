//! Filesystem integration tests.
//!
//! These tests exercise the path-based `FileSystem` wrapper used by the shell.
//! They intentionally use separate top-level directories so that test ordering
//! does not matter and a failed test does not normally corrupt the state needed
//! by another test.

pub use oxenna_test_macro::test;

use crate::console_println;

use alloc::vec;
use crate::{
    fs::{
        Entry,
        FileSystem,
        FileType,
        FsError,
        FS,
    },
    test::TestResult,
};

const MOUNT_FAILED: &str = "filesystem mount failed";
const SETUP_FAILED: &str = "filesystem setup failed";
const CLEANUP_FAILED: &str = "filesystem cleanup failed";
const CONTENT_MISMATCH: &str = "file contents do not match";
const EXPECTED_NOT_FOUND: &str = "expected NotFound";
const EXPECTED_EXISTS: &str = "expected Exists";
const EXPECTED_NOT_DIR: &str = "expected NotDir";
const EXPECTED_IS_DIR: &str = "expected IsDir";
const EXPECTED_NOT_EMPTY: &str = "expected NotEmpty";
const EXPECTED_INVALID: &str = "expected Invalid";
const EXPECTED_NAME_TOO_LONG: &str = "expected NameTooLong";

fn mount() -> bool {
    FS.lock().mount().is_ok()
}

fn cleanup(path: &str) {
    // Best-effort cleanup. Tests use unique top-level directories, so this
    // intentionally does not try to recursively delete arbitrary filesystem
    // contents.
    let _ = FS.lock().remove(path);
    let _ = FS.lock().rmdir(path);
}

fn make_dir(path: &str) -> bool {
    FS.lock().mkdir(path).is_ok()
}

fn contains_entry(path: &str, name: &str, want_dir: bool) -> bool {
    match FS.lock().list_entries(path) {
        Ok(entries) => entries.into_iter().any(|entry| match entry {
            Entry::Dir(n) => want_dir && n == name,
            Entry::File(n) => !want_dir && n == name,
        }),
        Err(_) => false,
    }
}

// ============================================================
// Mount / root
// ============================================================

#[test]
pub fn filesystem_mount() -> TestResult {
    if mount() {
        TestResult::Pass
    } else {
        TestResult::Fail(MOUNT_FAILED)
    }
}

#[test]
pub fn filesystem_root_exists() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    match FS.lock().stat("/") {
        Ok(stat) if stat.file_type == FileType::Directory => TestResult::Pass,
        _ => TestResult::Fail("root is not a directory"),
    }
}

#[test]
pub fn filesystem_root_listing() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    match FS.lock().list_entries("/") {
        Ok(_) => TestResult::Pass,
        Err(_) => TestResult::Fail("could not read root directory"),
    }
}

// ============================================================
// Basic file lifecycle
// ============================================================

#[test]
pub fn filesystem_touch_create() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_touch";
    cleanup(path);

    let result = FS.lock().touch(path);
    if result.is_err() {
        return TestResult::Fail("touch failed");
    }

    let ok = match FS.lock().stat(path) {
        Ok(stat) => {
            stat.file_type == FileType::Regular
                && stat.size == 0
                && stat.mode & 0o777 == 0o644
        }
        Err(_) => false,
    };

    cleanup(path);

    if ok {
        TestResult::Pass
    } else {
        TestResult::Fail("created file has incorrect metadata")
    }
}

#[test]
pub fn filesystem_touch_existing_is_idempotent() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_touch_existing";
    cleanup(path);

    if FS.lock().touch(path).is_err() {
        return TestResult::Fail("initial touch failed");
    }

    let result = FS.lock().touch(path);
    cleanup(path);

    if result.is_ok() {
        TestResult::Pass
    } else {
        TestResult::Fail("touch on an existing file failed")
    }
}

#[test]
pub fn filesystem_write_and_read() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_write_read";
    cleanup(path);

    let data = b"Hello from the OXENNA filesystem.";

    if FS.lock().write_file(path, data).is_err() {
        return TestResult::Fail("write_file failed");
    }

    let read = FS.lock().read_file(path);
    cleanup(path);

    match read {
        Ok(got) if got.as_slice() == data => TestResult::Pass,
        Ok(_) => TestResult::Fail(CONTENT_MISMATCH),
        Err(_) => TestResult::Fail("read_file failed"),
    }
}

#[test]
pub fn filesystem_empty_file() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_empty";
    cleanup(path);

    if FS.lock().write_file(path, &[]).is_err() {
        return TestResult::Fail("empty write failed");
    }

    let result = FS.lock().read_file(path);
    cleanup(path);

    match result {
        Ok(data) if data.is_empty() => TestResult::Pass,
        _ => TestResult::Fail("empty file could not be read as empty"),
    }
}

#[test]
pub fn filesystem_overwrite_truncates() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_truncate";
    cleanup(path);

    let first = b"this is a long first version";
    let second = b"short";

    if FS.lock().write_file(path, first).is_err() {
        return TestResult::Fail("initial write failed");
    }

    if FS.lock().write_file(path, second).is_err() {
        return TestResult::Fail("overwrite failed");
    }

    let read = FS.lock().read_file(path);
    let stat = FS.lock().stat(path);
    cleanup(path);

    match (read, stat) {
        (Ok(data), Ok(stat))
            if data.as_slice() == second && stat.size == second.len() as u64 =>
        {
            TestResult::Pass
        }
        _ => TestResult::Fail("overwrite did not truncate correctly"),
    }
}

// ============================================================
// Directory operations
// ============================================================

#[test]
pub fn filesystem_mkdir() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_mkdir";
    cleanup(path);

    let created = FS.lock().mkdir(path).is_ok();
    let stat = FS.lock().stat(path);
    cleanup(path);

    if !created {
        return TestResult::Fail("mkdir failed");
    }

    match stat {
        Ok(stat) if stat.file_type == FileType::Directory => TestResult::Pass,
        _ => TestResult::Fail("created directory has wrong type"),
    }
}

#[test]
pub fn filesystem_mkdir_duplicate() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_mkdir_duplicate";
    cleanup(path);

    if FS.lock().mkdir(path).is_err() {
        return TestResult::Fail("initial mkdir failed");
    }

    let result = FS.lock().mkdir(path);
    cleanup(path);

    match result {
        Err(FsError::Exists) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_EXISTS),
    }
}

#[test]
pub fn filesystem_nested_directories() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let root = "/__fstest_nested";
    let a = "/__fstest_nested/a";
    let b = "/__fstest_nested/a/b";
    let file = "/__fstest_nested/a/b/file";

    cleanup(root);

    if !make_dir(root) || !make_dir(a) || !make_dir(b) {
        return TestResult::Fail("nested mkdir failed");
    }

    if FS.lock().write_file(file, b"nested").is_err() {
        return TestResult::Fail("nested file creation failed");
    }

    let read = FS.lock().read_file(file);
    cleanup(file);
    cleanup(b);
    cleanup(a);
    cleanup(root);

    match read {
        Ok(data) if data.as_slice() == b"nested" => TestResult::Pass,
        _ => TestResult::Fail("nested file could not be read"),
    }
}

#[test]
pub fn filesystem_directory_listing() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let root = "/__fstest_listing";
    let dir = "/__fstest_listing/subdir";
    let file = "/__fstest_listing/file";

    cleanup(root);

    if !make_dir(root) || !make_dir(dir) {
        return TestResult::Fail("listing setup mkdir failed");
    }

    if FS.lock().write_file(file, b"x").is_err() {
        return TestResult::Fail("listing setup file creation failed");
    }

    let dirs = contains_entry(root, "subdir", true);
    let files = contains_entry(root, "file", false);

    cleanup(file);
    cleanup(dir);
    cleanup(root);

    if dirs && files {
        TestResult::Pass
    } else {
        TestResult::Fail("directory listing is missing entries")
    }
}

#[test]
pub fn filesystem_rmdir_empty() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_rmdir";
    cleanup(path);

    if !make_dir(path) {
        return TestResult::Fail("mkdir failed");
    }

    let result = FS.lock().rmdir(path);

    if result.is_ok() && matches!(FS.lock().stat(path), Err(FsError::NotFound)) {
        TestResult::Pass
    } else {
        TestResult::Fail("empty directory was not removed")
    }
}

#[test]
pub fn filesystem_rmdir_nonempty() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let dir = "/__fstest_rmdir_nonempty";
    let file = "/__fstest_rmdir_nonempty/file";

    cleanup(dir);

    if !make_dir(dir) || FS.lock().write_file(file, b"x").is_err() {
        return TestResult::Fail("setup failed");
    }

    let result = FS.lock().rmdir(dir);

    cleanup(file);
    cleanup(dir);

    match result {
        Err(FsError::NotEmpty) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_NOT_EMPTY),
    }
}

// ============================================================
// Removal semantics
// ============================================================

#[test]
pub fn filesystem_rm_file() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_rm";
    cleanup(path);

    if FS.lock().write_file(path, b"remove me").is_err() {
        return TestResult::Fail("setup failed");
    }

    let removed = FS.lock().remove(path).is_ok();
    let missing = matches!(FS.lock().stat(path), Err(FsError::NotFound));

    if removed && missing {
        TestResult::Pass
    } else {
        TestResult::Fail("file was not removed")
    }
}

#[test]
pub fn filesystem_rm_missing() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_missing";
    cleanup(path);

    match FS.lock().remove(path) {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_NOT_FOUND),
    }
}

#[test]
pub fn filesystem_rm_directory_rejected() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_rm_dir";
    cleanup(path);

    if !make_dir(path) {
        return TestResult::Fail("setup mkdir failed");
    }

    let result = FS.lock().remove(path);
    cleanup(path);

    match result {
        Err(FsError::IsDir) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_IS_DIR),
    }
}

#[test]
pub fn filesystem_read_directory_rejected() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_read_dir";
    cleanup(path);

    if !make_dir(path) {
        return TestResult::Fail("setup mkdir failed");
    }

    let result = FS.lock().read_file(path);
    cleanup(path);

    match result {
        Err(FsError::IsDir) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_IS_DIR),
    }
}

// ============================================================
// Rename
// ============================================================

#[test]
pub fn filesystem_rename_file() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let old = "/__fstest_rename_old";
    let new = "/__fstest_rename_new";

    cleanup(old);
    cleanup(new);

    if FS.lock().write_file(old, b"rename me").is_err() {
        return TestResult::Fail("setup failed");
    }

    let result = FS.lock().rename(old, new);
    let old_missing = matches!(FS.lock().stat(old), Err(FsError::NotFound));
    let new_data = FS.lock().read_file(new);

    cleanup(old);
    cleanup(new);

    match (result, old_missing, new_data) {
        (Ok(()), true, Ok(data)) if data.as_slice() == b"rename me" => TestResult::Pass,
        _ => TestResult::Fail("file rename failed"),
    }
}

#[test]
pub fn filesystem_rename_directory() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let old = "/__fstest_rename_dir_old";
    let new = "/__fstest_rename_dir_new";
    let file = "/__fstest_rename_dir_old/file";
    let new_file = "/__fstest_rename_dir_new/file";

    // Make sure stale entries from a previous failed run are gone.
    cleanup(file);
    cleanup(new_file);
    cleanup(old);
    cleanup(new);

    if !make_dir(old) {
        return TestResult::Fail("setup mkdir failed");
    }

    if FS.lock().write_file(file, b"inside").is_err() {
        cleanup(old);
        return TestResult::Fail("setup file creation failed");
    }

    console_println!("renaming {} -> {}", old, new);

    let result = FS.lock().rename(old, new);

    if let Err(e) = result {
        console_println!("directory rename returned: {:?}", e);

        // Inspect what exists at the destination.
        match FS.lock().stat(new) {
            Ok(stat) => {
                console_println!(
                    "destination exists: type={:?}",
                    stat.file_type
                );
            }
            Err(stat_err) => {
                console_println!(
                    "destination stat returned: {:?}",
                    stat_err
                );
            }
        }

        cleanup(file);
        cleanup(old);
        cleanup(new);

        return TestResult::Fail("directory rename failed");
    }

    let old_missing = matches!(
        FS.lock().stat(old),
        Err(FsError::NotFound)
    );

    let new_is_dir = matches!(
        FS.lock().stat(new),
        Ok(stat) if stat.file_type == FileType::Directory
    );

    let data = FS.lock().read_file(new_file);

    cleanup(new_file);
    cleanup(new);
    cleanup(old);

    if !old_missing {
        return TestResult::Fail("old directory still exists");
    }

    if !new_is_dir {
        return TestResult::Fail("renamed directory does not exist");
    }

    match data {
        Ok(data) if data.as_slice() == b"inside" => TestResult::Pass,
        Ok(_) => TestResult::Fail("file contents changed after directory rename"),
        Err(_) => TestResult::Fail("file inside renamed directory is missing"),
    }
}

#[test]
pub fn filesystem_rename_into_own_subtree_rejected() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let root = "/__fstest_rename_loop";
    let child = "/__fstest_rename_loop/child";
    let target = "/__fstest_rename_loop/child/moved";

    cleanup(root);

    if !make_dir(root) || !make_dir(child) {
        return TestResult::Fail("setup failed");
    }

    let result = FS.lock().rename(root, target);

    cleanup(child);
    cleanup(root);

    match result {
        Err(FsError::Invalid) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_INVALID),
    }
}

// ============================================================
// Path handling
// ============================================================

#[test]
pub fn filesystem_dot_path() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let dir = "/__fstest_dot";
    let file = "/__fstest_dot/file";

    cleanup(dir);

    if !make_dir(dir) {
        return TestResult::Fail("setup mkdir failed");
    }

    if FS.lock().write_file("/__fstest_dot/./file", b"dot").is_err() {
        return TestResult::Fail("dot path write failed");
    }

    let data = FS.lock().read_file(file);

    cleanup(file);
    cleanup(dir);

    match data {
        Ok(data) if data.as_slice() == b"dot" => TestResult::Pass,
        _ => TestResult::Fail("dot path did not resolve"),
    }
}

#[test]
pub fn filesystem_repeated_slashes() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let dir = "/__fstest_slashes";
    let file = "/__fstest_slashes/file";

    cleanup(dir);

    if !make_dir(dir) {
        return TestResult::Fail("setup mkdir failed");
    }

    if FS.lock().write_file("//__fstest_slashes///file", b"slashes").is_err() {
        return TestResult::Fail("repeated slash write failed");
    }

    let data = FS.lock().read_file(file);

    cleanup(file);
    cleanup(dir);

    match data {
        Ok(data) if data.as_slice() == b"slashes" => TestResult::Pass,
        _ => TestResult::Fail("repeated slashes did not resolve"),
    }
}

#[test]
pub fn filesystem_parent_path() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let root = "/__fstest_parent";
    let child = "/__fstest_parent/child";
    let file = "/__fstest_parent/file";

    cleanup(root);

    if !make_dir(root) || !make_dir(child) {
        return TestResult::Fail("setup failed");
    }

    if FS.lock().write_file("/__fstest_parent/child/../file", b"parent").is_err() {
        return TestResult::Fail("parent path write failed");
    }

    let data = FS.lock().read_file(file);

    cleanup(file);
    cleanup(child);
    cleanup(root);

    match data {
        Ok(data) if data.as_slice() == b"parent" => TestResult::Pass,
        _ => TestResult::Fail("parent path did not resolve"),
    }
}

#[test]
pub fn filesystem_root_parent_is_root() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let data = FS.lock().read_file("/../..");

    match data {
        Err(FsError::IsDir) => TestResult::Pass,
        _ => TestResult::Fail("parent traversal above root behaved unexpectedly"),
    }
}

// ============================================================
// Metadata
// ============================================================

#[test]
pub fn filesystem_stat_regular_file() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_stat_file";
    cleanup(path);

    let data = b"stat metadata";
    if FS.lock().write_file(path, data).is_err() {
        return TestResult::Fail("setup failed");
    }

    let stat = FS.lock().stat(path);
    cleanup(path);

    match stat {
        Ok(stat)
            if stat.file_type == FileType::Regular
                && stat.size == data.len() as u64
                && stat.mode & 0o777 == 0o644
                && stat.links >= 1 =>
        {
            TestResult::Pass
        }
        _ => TestResult::Fail("regular file metadata is incorrect"),
    }
}

#[test]
pub fn filesystem_stat_directory() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_stat_dir";
    cleanup(path);

    if !make_dir(path) {
        return TestResult::Fail("mkdir failed");
    }

    let stat = FS.lock().stat(path);
    cleanup(path);

    match stat {
        Ok(stat)
            if stat.file_type == FileType::Directory
                && stat.mode & 0o777 == 0o755 =>
        {
            TestResult::Pass
        }
        _ => TestResult::Fail("directory metadata is incorrect"),
    }
}

// ============================================================
// Name validation / type errors
// ============================================================

#[test]
pub fn filesystem_name_length_limit() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let valid_name = alloc::format!("/{}", "a".repeat(255));
    let invalid_name = alloc::format!("/{}", "b".repeat(256));

    cleanup(&valid_name);
    cleanup(&invalid_name);

    let valid = FS.lock().write_file(&valid_name, b"valid");
    let invalid = FS.lock().write_file(&invalid_name, b"invalid");

    cleanup(&valid_name);
    cleanup(&invalid_name);

    if valid.is_err() {
        return TestResult::Fail("255-byte filename was rejected");
    }

    match invalid {
        Err(FsError::NameTooLong) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_NAME_TOO_LONG),
    }
}

#[test]
pub fn filesystem_missing_parent() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_no_such_parent/file";
    cleanup("/__fstest_no_such_parent");

    match FS.lock().write_file(path, b"x") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail(EXPECTED_NOT_FOUND),
    }
}

// ============================================================
// Larger I/O
// ============================================================

#[test]
pub fn filesystem_multiblock_io() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_multiblock";
    cleanup(path);

    // Larger than a 1 KiB ext2 block and large enough to exercise multiple
    // direct data blocks.
    let mut data = vec![0u8; 8192];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = ((i * 31 + 7) & 0xff) as u8;
    }

    if FS.lock().write_file(path, &data).is_err() {
        return TestResult::Fail("multi-block write failed");
    }

    let read = FS.lock().read_file(path);
    cleanup(path);

    match read {
        Ok(got) if got == data => TestResult::Pass,
        Ok(_) => TestResult::Fail(CONTENT_MISMATCH),
        Err(_) => TestResult::Fail("multi-block read failed"),
    }
}

#[test]
pub fn filesystem_binary_data() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_binary";
    cleanup(path);

    let data: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x7f, 0x80, 0x81, 0xfe,
        0xff, 0x00, 0xaa, 0x55, 0x10, 0x20, 0x30, 0x40,
        0x41, 0x42, 0x43, 0x44, 0x80, 0x90, 0xa0, 0xb0,
        0xc0, 0xd0, 0xe0, 0xf0, 0xff, 0xee, 0xdd, 0xcc,
    ];

    if FS.lock().write_file(path, &data).is_err() {
        return TestResult::Fail("binary write failed");
    }

    let read = FS.lock().read_file(path);
    cleanup(path);

    match read {
        Ok(got) if got.as_slice() == data => TestResult::Pass,
        Ok(_) => TestResult::Fail("binary data was modified"),
        Err(_) => TestResult::Fail("binary read failed"),
    }
}

// ============================================================
// Synchronization
// ============================================================

#[test]
pub fn filesystem_sync() -> TestResult {
    if !mount() {
        return TestResult::Fail(MOUNT_FAILED);
    }

    let path = "/__fstest_sync";
    cleanup(path);

    if FS.lock().write_file(path, b"sync me").is_err() {
        return TestResult::Fail("setup write failed");
    }

    let synced = FS.lock().sync().is_ok();
    let data = FS.lock().read_file(path);

    cleanup(path);
    let _ = FS.lock().sync();

    match (synced, data) {
        (true, Ok(data)) if data.as_slice() == b"sync me" => TestResult::Pass,
        _ => TestResult::Fail("filesystem sync failed"),
    }
}
