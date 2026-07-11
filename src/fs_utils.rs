use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PRIVATE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically replace a file by writing to a sibling temp file first.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir '{}'", parent.display()))?;
    }

    let tmp_path = temp_path(path);
    let mut file = File::create(&tmp_path)
        .with_context(|| format!("failed to create temp file '{}'", tmp_path.display()))?;
    file.write_all(content)
        .with_context(|| format!("failed to write temp file '{}'", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync temp file '{}'", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to rename temp file '{}' to '{}'",
            tmp_path.display(),
            path.display()
        )
    })
}

/// Atomically replace a secret-bearing file. The temporary file is created
/// exclusively; on Unix both it and the final file are owner-only (`0600`).
pub fn atomic_write_private(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent dir '{}'", parent.display()))?;

    let (tmp_path, mut file) = loop {
        let nonce = PRIVATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = private_temp_path(path, nonce);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp_path) {
            Ok(file) => break (tmp_path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create private temp file '{}'",
                        tmp_path.display()
                    )
                });
            }
        }
    };

    let result = (|| -> Result<()> {
        file.write_all(content).with_context(|| {
            format!("failed to write private temp file '{}'", tmp_path.display())
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!(
                        "failed to secure private temp file '{}'",
                        tmp_path.display()
                    )
                })?;
        }
        file.sync_all().with_context(|| {
            format!("failed to sync private temp file '{}'", tmp_path.display())
        })?;
        drop(file);
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to rename private temp file '{}' to '{}'",
                tmp_path.display(),
                path.display()
            )
        })?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync config directory '{}'", parent.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| "tmp".into());
    name.push(".tmp");
    path.with_file_name(name)
}

fn private_temp_path(path: &Path, nonce: u64) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "config".into());
    name.push(format!(".{}.{}.tmp", std::process::id(), nonce));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_atomic_write_replaces_content_with_owner_only_file() {
        let nonce = PRIVATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "epub_reader_private_write_{}_{}",
            std::process::id(),
            nonce
        ));
        std::fs::write(&path, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        atomic_write_private(&path, b"new-secret-config").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new-secret-config");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(&path);
    }
}
