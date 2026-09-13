mod tempfs;

extern crate alloc;
use alloc::{string::String, vec::Vec};
use spin::Mutex;

use crate::fs::tempfs::TmpFs;

pub static FS: Mutex<TmpFs> = Mutex::new(TmpFs::new());

pub enum Entry {
    File(String),
    Dir(String),
}

#[expect(unused)]
#[derive(Debug)]
pub enum FsError {
    NotFound,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
}

pub trait FileSystem {
    fn read(&self, path: &str) -> Result<Vec<u8>, FsError>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsError>;
    fn list(&self, path: &str) -> Result<Vec<String>, FsError>;
    fn remove(&mut self, path: &str) -> Result<(), FsError>;
}

// ============================================================
// Tests
// ============================================================

use crate::test::{test, TestResult};

#[test]
fn test_touch_creates_root_file() -> TestResult {
    let mut fs = FS.lock();

    if fs.touch("/test_touch_root.txt").is_err() {
        return TestResult::Fail("touch on root failed unexpectedly");
    }

    match fs.read("/test_touch_root.txt") {
        Ok(data) if data.is_empty() => TestResult::Pass,
        Ok(_) => TestResult::Fail("new file was not empty"),
        Err(_) => TestResult::Fail("could not read freshly touched file"),
    }
}

#[test]
fn test_touch_missing_parent_fails() -> TestResult {
    let mut fs = FS.lock();

    match fs.touch("/test_touch_missing_parent/file.txt") {
        Err(FsError::NotFound) => TestResult::Pass,
        Err(_) => TestResult::Fail("wrong error kind for missing parent"),
        Ok(_) => TestResult::Fail("touch succeeded with missing parent"),
    }
}

#[test]
fn test_touch_on_directory_fails() -> TestResult {
    let mut fs = FS.lock();

    if fs.mkdir("/test_touch_on_dir").is_err() {
        return TestResult::Fail("setup mkdir failed");
    }

    match fs.touch("/test_touch_on_dir") {
        Err(FsError::IsADirectory) => TestResult::Pass,
        _ => TestResult::Fail("touch on a directory did not fail as IsADirectory"),
    }
}

#[test]
fn test_mkdir_creates_dir() -> TestResult {
    let mut fs = FS.lock();

    if fs.mkdir("/test_mkdir_new").is_err() {
        return TestResult::Fail("mkdir failed unexpectedly");
    }

    match fs.list_entries("/") {
        Ok(entries) => {
            let found = entries
                .iter()
                .any(|e| matches!(e, Entry::Dir(name) if name == "test_mkdir_new"));

            if found {
                TestResult::Pass
            } else {
                TestResult::Fail("new directory not visible in parent listing")
            }
        }
        Err(_) => TestResult::Fail("could not list root"),
    }
}

#[test]
fn test_mkdir_duplicate_fails() -> TestResult {
    let mut fs = FS.lock();

    if fs.mkdir("/test_mkdir_dup").is_err() {
        return TestResult::Fail("setup mkdir failed");
    }

    match fs.mkdir("/test_mkdir_dup") {
        Err(FsError::AlreadyExists) => TestResult::Pass,
        _ => TestResult::Fail("duplicate mkdir did not fail as AlreadyExists"),
    }
}

#[test]
fn test_mkdir_missing_parent_fails() -> TestResult {
    let mut fs = FS.lock();

    match fs.mkdir("/test_mkdir_missing_parent/child") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail("mkdir with missing parent did not fail as NotFound"),
    }
}

#[test]
fn test_ls_lists_files_and_dirs() -> TestResult {
    let mut fs = FS.lock();

    if fs.mkdir("/test_ls_dir").is_err()
        || fs.touch("/test_ls_dir/a.txt").is_err()
        || fs.touch("/test_ls_dir/b.txt").is_err()
        || fs.mkdir("/test_ls_dir/sub").is_err()
    {
        return TestResult::Fail("setup for ls test failed");
    }

    match fs.list("/test_ls_dir") {
        Ok(mut names) => {
            names.sort();

            if names == ["a.txt", "b.txt", "sub/"] {
                TestResult::Pass
            } else {
                TestResult::Fail("ls did not return expected entries")
            }
        }
        Err(_) => TestResult::Fail("ls on valid directory failed"),
    }
}

#[test]
fn test_ls_missing_dir_fails() -> TestResult {
    let fs = FS.lock();

    match fs.list("/test_ls_does_not_exist") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail("ls on missing directory did not fail as NotFound"),
    }
}

#[test]
fn test_write_read_roundtrip() -> TestResult {
    let mut fs = FS.lock();

    let data = b"hello from oxenna";

    if fs.write("/test_write_read.txt", data).is_err() {
        return TestResult::Fail("write failed unexpectedly");
    }

    match fs.read("/test_write_read.txt") {
        Ok(bytes) if bytes == data => TestResult::Pass,
        Ok(_) => TestResult::Fail("read data did not match written data"),
        Err(_) => TestResult::Fail("read failed after successful write"),
    }
}

#[test]
fn test_write_on_directory_fails() -> TestResult {
    let mut fs = FS.lock();

    if fs.mkdir("/test_write_on_dir").is_err() {
        return TestResult::Fail("setup mkdir failed");
    }

    match fs.write("/test_write_on_dir", b"nope") {
        Err(FsError::IsADirectory) => TestResult::Pass,
        _ => TestResult::Fail("write on a directory did not fail as IsADirectory"),
    }
}

#[test]
fn test_remove_file() -> TestResult {
    let mut fs = FS.lock();

    if fs.write("/test_remove.txt", b"bye").is_err() {
        return TestResult::Fail("setup write failed");
    }

    if fs.remove("/test_remove.txt").is_err() {
        return TestResult::Fail("remove failed unexpectedly");
    }

    match fs.read("/test_remove.txt") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail("file still readable after remove"),
    }
}

#[test]
fn test_remove_missing_file_fails() -> TestResult {
    let mut fs = FS.lock();

    match fs.remove("/test_remove_missing.txt") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail("removing a missing file did not fail as NotFound"),
    }
}

#[test]
fn test_read_missing_file_fails() -> TestResult {
    let fs = FS.lock();

    match fs.read("/test_read_missing.txt") {
        Err(FsError::NotFound) => TestResult::Pass,
        _ => TestResult::Fail("reading a missing file did not fail as NotFound"),
    }
}
