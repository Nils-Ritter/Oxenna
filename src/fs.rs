#[cfg(feature = "fs_ext2")]
pub mod ext2;

extern crate alloc;

use alloc::{string::String, vec::Vec};
use spin::Mutex;

#[cfg(feature = "fs_ext2")]
use crate::drivers::ata::AtaDrive;
pub use crate::fs::ext2::{Ext2, FileType, FsError, ROOT_INO, Stat};

pub use ext2::{DirEntry as Ext2DirEntry, FileType as Ext2FileType, FsUsage};

/// The kernel's filesystem.
///
/// This is a thin path-based wrapper around the real on-disk ext2
/// implementation. The first ATA disk found by the PIO driver is mounted
/// lazily on the first filesystem operation.
pub struct FileSystem {
    inner: Option<Ext2<AtaDrive>>,
}

impl FileSystem {
    pub const fn new() -> Self {
        Self { inner: None }
    }

    fn fs(&mut self) -> Result<&mut Ext2<AtaDrive>, FsError> {
        if self.inner.is_none() {
            let drives = unsafe { AtaDrive::probe_all() };
            if drives.is_empty() {
                return Err(FsError::NotFound);
            }

            let mut last_error = FsError::NotFound;
            for drive in drives {
                match Ext2::mount(drive) {
                    Ok(fs) => {
                        self.inner = Some(fs);
                        break;
                    }
                    Err(e) => last_error = e,
                }
            }

            if self.inner.is_none() {
                return Err(last_error);
            }
        }

        self.inner.as_mut().ok_or(FsError::NotFound)
    }

    pub fn mount(&mut self) -> Result<(), FsError> {
        let _ = self.fs()?;
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), FsError> {
        self.fs()?.sync()
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        fs.read_to_vec(ino)
    }

    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        self.fs()?.write_file(ROOT_INO, path, data)?;
        Ok(())
    }

    pub fn list(&mut self, path: &str) -> Result<Vec<String>, FsError> {
        let entries = self.list_entries(path)?;
        Ok(entries
            .into_iter()
            .map(|entry| match entry {
                Entry::Dir(mut name) => {
                    name.push('/');
                    name
                }
                Entry::File(name) => name,
            })
            .collect())
    }

    pub fn list_entries(&mut self, path: &str) -> Result<Vec<Entry>, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        let entries = fs.readdir(ino)?;

        Ok(entries
            .into_iter()
            .filter(|entry| entry.name != "." && entry.name != "..")
            .map(|entry| {
                if entry.file_type == FileType::Directory {
                    Entry::Dir(entry.name)
                } else {
                    Entry::File(entry.name)
                }
            })
            .collect())
    }

    pub fn remove(&mut self, path: &str) -> Result<(), FsError> {
        let fs = self.fs()?;
        let (dir, name) = fs.resolve_parent(ROOT_INO, path)?;
        fs.unlink(dir, name)
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let fs = self.fs()?;
        let (dir, name) = fs.resolve_parent(ROOT_INO, path)?;
        fs.mkdir(dir, name, 0o755)?;
        Ok(())
    }

    pub fn touch(&mut self, path: &str) -> Result<(), FsError> {
        let fs = self.fs()?;
        let (dir, name) = fs.resolve_parent(ROOT_INO, path)?;

        match fs.lookup(dir, name) {
            Ok(ino) => {
                let stat = fs.stat(ino)?;
                if stat.file_type == FileType::Directory {
                    Err(FsError::IsDir)
                } else {
                    Ok(())
                }
            }
            Err(FsError::NotFound) => {
                fs.create(dir, name, 0o644)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn rmdir(&mut self, path: &str) -> Result<(), FsError> {
        let fs = self.fs()?;
        let (dir, name) = fs.resolve_parent(ROOT_INO, path)?;
        fs.rmdir(dir, name)
    }

    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), FsError> {
        let fs = self.fs()?;
        let (old_dir, old_name) = fs.resolve_parent(ROOT_INO, old)?;
        let (new_dir, new_name) = fs.resolve_parent(ROOT_INO, new)?;
        fs.rename(old_dir, old_name, new_dir, new_name)
    }

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        self.read(path)
    }

    /// Read from an open-file style offset without materializing the whole file.
    pub fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        fs.read(ino, offset, buffer)
    }

    /// Write at an open-file style offset.
    pub fn write_at(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        fs.write(ino, offset, data)
    }

    /// Return directory entries including inode/type information for getdents.
    pub fn readdir(&mut self, path: &str) -> Result<Vec<Ext2DirEntry>, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        fs.readdir(ino)
    }

    pub fn readlink(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, false)?;
        fs.readlink(ino)
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        self.write(path, data)
    }

    pub fn stat(&mut self, path: &str) -> Result<Stat, FsError> {
        let fs = self.fs()?;
        let ino = fs.resolve(ROOT_INO, path, true)?;
        fs.stat(ino)
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());

#[derive(Debug, Clone)]
pub enum Entry {
    File(String),
    Dir(String),
}

// Keep the old trait as the shell-facing generic filesystem interface.
pub trait FileSystemTrait {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsError>;
    fn list(&mut self, path: &str) -> Result<Vec<String>, FsError>;
    fn remove(&mut self, path: &str) -> Result<(), FsError>;
}

impl FileSystemTrait for FileSystem {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        FileSystem::read(self, path)
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        FileSystem::write(self, path, data)
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>, FsError> {
        FileSystem::list(self, path)
    }

    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        FileSystem::remove(self, path)
    }
}
