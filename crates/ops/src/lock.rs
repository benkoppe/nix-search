use std::fs::{self, File, OpenOptions, TryLockError};

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};

const UPDATE_LOCK_FILENAME: &str = "update.lock";

#[derive(Debug)]
pub struct UpdateLock {
    path: Utf8PathBuf,
    _file: File,
}

impl UpdateLock {
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }
}

pub fn update_lock_path(index_dir: &Utf8Path) -> Utf8PathBuf {
    index_dir.join(UPDATE_LOCK_FILENAME)
}

pub fn acquire_update_lock(index_dir: &Utf8Path) -> Result<UpdateLock> {
    let path = prepare_lock_file(index_dir)?;
    let file = open_lock_file(&path)?;

    tracing::info!("waiting for maintenance lock {path}");
    file.lock()
        .with_context(|| format!("failed to acquire maintenance lock {path}"))?;
    tracing::info!("acquired maintenance lock {path}");

    Ok(UpdateLock { path, _file: file })
}

pub fn try_acquire_update_lock(index_dir: &Utf8Path) -> Result<Option<UpdateLock>> {
    let path = prepare_lock_file(index_dir)?;
    let file = open_lock_file(&path)?;

    match file.try_lock() {
        Ok(()) => {
            tracing::info!("acquired maintenance lock {path}");
            Ok(Some(UpdateLock { path, _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("failed to acquire maintenance lock {path}"))
        }
    }
}

fn prepare_lock_file(index_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    fs::create_dir_all(index_dir)
        .with_context(|| format!("failed to create index dir {index_dir}"))?;

    Ok(update_lock_path(index_dir))
}

fn open_lock_file(path: &Utf8Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open maintenance lock {path}"))
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::{try_acquire_update_lock, update_lock_path};

    fn utf8_path(path: std::path::PathBuf) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path).expect("test path must be valid UTF-8")
    }

    #[test]
    fn update_lock_path_uses_index_dir() {
        let dir = tempdir().unwrap();
        let dir = utf8_path(dir.path().to_path_buf());

        assert_eq!(update_lock_path(&dir), dir.join("update.lock"));
    }

    #[test]
    fn try_acquire_update_lock_reports_contention() {
        let dir = tempdir().unwrap();
        let dir = utf8_path(dir.path().to_path_buf());

        let first = try_acquire_update_lock(&dir).unwrap();
        assert!(first.is_some());

        let second = try_acquire_update_lock(&dir).unwrap();
        assert!(second.is_none());

        drop(first);

        let third = try_acquire_update_lock(&dir).unwrap();
        assert!(third.is_some());
    }
}
