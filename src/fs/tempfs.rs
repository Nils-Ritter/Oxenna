use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::String,
    vec::Vec,
};

use crate::fs::{Entry, FileSystem, FsError};

pub struct TmpFs {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

impl TmpFs {
    pub const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
        }
    }

    fn is_dir(&self, path: &str) -> bool {
        path == "/" || self.dirs.contains(path)
    }

    fn parent_of(path: &str) -> String {
        match path.trim_end_matches('/').rfind('/') {
            Some(0) => String::from("/"),
            Some(idx) => String::from(&path[..idx]),
            None => String::from("/"),
        }
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        if path == "/" || self.is_dir(path) || self.files.contains_key(path) {
            return Err(FsError::AlreadyExists);
        }

        let parent = Self::parent_of(path);

        if !self.is_dir(&parent) {
            return Err(FsError::NotFound);
        }

        self.dirs.insert(String::from(path));

        Ok(())
    }

    pub fn touch(&mut self, path: &str) -> Result<(), FsError> {
        if self.is_dir(path) {
            return Err(FsError::IsADirectory);
        }

        let parent = Self::parent_of(path);

        if !self.is_dir(&parent) {
            return Err(FsError::NotFound);
        }

        self.files
            .entry(String::from(path))
            .or_insert_with(Vec::new);

        Ok(())
    }

    pub fn list_entries(&self, path: &str) -> Result<Vec<Entry>, FsError> {
        if !self.is_dir(path) {
            return Err(FsError::NotFound);
        }

        let mut entries = Vec::new();

        for dir in &self.dirs {
            if Self::parent_of(dir) == path {
                entries.push(Entry::Dir(Self::last_segment(dir)));
            }
        }

        for file in self.files.keys() {
            if Self::parent_of(file) == path {
                entries.push(Entry::File(Self::last_segment(file)));
            }
        }

        Ok(entries)
    }

    fn last_segment(path: &str) -> String {
        String::from(
            path.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path),
        )
    }
}

impl FileSystem for TmpFs {
    fn read(&self, path: &str) -> Result<Vec<u8>, FsError> {
        self.files.get(path).cloned().ok_or(FsError::NotFound)
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        if self.is_dir(path) {
            return Err(FsError::IsADirectory);
        }

        self.files.insert(String::from(path), Vec::from(data));

        Ok(())
    }

    fn list(&self, path: &str) -> Result<Vec<String>, FsError> {
        Ok(self
            .list_entries(path)?
            .into_iter()
            .map(|e| match e {
                Entry::File(name) => name,
                Entry::Dir(name) => format!("{name}/"),
            })
            .collect())
    }

    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        self.files.remove(path).map(|_| ()).ok_or(FsError::NotFound)
    }
}
