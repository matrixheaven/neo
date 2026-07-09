use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context as _, bail};
use serde::{Serialize, de::DeserializeOwned};

const LOCK_RETRY_ATTEMPTS: usize = 50;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);

pub(crate) fn read_or_default<T>(path: &Path, label: &str) -> anyhow::Result<T>
where
    T: Default + DeserializeOwned,
{
    match read_json(path, label)? {
        Some(data) => Ok(data),
        None => Ok(T::default()),
    }
}

pub(crate) fn update<T>(path: &Path, label: &str, update: impl FnOnce(&mut T)) -> anyhow::Result<()>
where
    T: Default + DeserializeOwned + Serialize,
{
    let parent = path
        .parent()
        .with_context(|| format!("{label} store has no parent"))?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create {label} store directory {}",
            parent.display()
        )
    })?;
    let _lock = StoreLock::acquire(path)?;
    let mut data = read_or_repair_default(path, label)?;
    update(&mut data);
    write_json(path, label, &data)
}

fn read_json<T>(path: &Path, label: &str) -> anyhow::Result<Option<T>>
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read {label} store {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<T>(&content) {
        Ok(data) => Ok(Some(data)),
        Err(err) => {
            tracing::warn!(
                "{label} store {} is corrupted ({err}); treating it as empty.",
                path.display()
            );
            Ok(None)
        }
    }
}

fn read_or_repair_default<T>(path: &Path, label: &str) -> anyhow::Result<T>
where
    T: Default + DeserializeOwned,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read {label} store {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }
    match serde_json::from_str::<T>(&content) {
        Ok(data) => Ok(data),
        Err(err) => {
            let backup = path.with_extension("json.bak");
            if backup.exists() {
                fs::remove_file(&backup).with_context(|| {
                    format!(
                        "failed to remove old {label} store backup {}",
                        backup.display()
                    )
                })?;
            }
            fs::rename(path, &backup).with_context(|| {
                format!(
                    "failed to back up corrupted {label} store to {}",
                    backup.display()
                )
            })?;
            tracing::warn!(
                "{label} store {} was corrupted ({err}); backed up to {}. Starting fresh.",
                path.display(),
                backup.display()
            );
            Ok(T::default())
        }
    }
}

fn write_json<T>(path: &Path, label: &str, data: &T) -> anyhow::Result<()>
where
    T: Serialize,
{
    let parent = path
        .parent()
        .with_context(|| format!("{label} store has no parent"))?;
    let content =
        serde_json::to_string_pretty(data).with_context(|| format!("serialize {label} store"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temporary {label} store file"))?;
    temp.write_all(content.as_bytes())
        .with_context(|| format!("write temporary {label} store file"))?;
    temp.persist(path)
        .map_err(|err| anyhow::anyhow!("failed to persist {label} store: {err}"))?;
    Ok(())
}

struct StoreLock {
    path: PathBuf,
}

impl StoreLock {
    fn acquire(store_path: &Path) -> anyhow::Result<Self> {
        let lock_path = store_path.with_extension("json.lock");
        for attempt in 0..LOCK_RETRY_ATTEMPTS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt + 1 == LOCK_RETRY_ATTEMPTS {
                        break;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to create store lock {}", lock_path.display())
                    });
                }
            }
        }
        bail!("timed out waiting for store lock {}", lock_path.display());
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
