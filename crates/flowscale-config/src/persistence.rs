use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// Replaces a file via a flushed sibling temp file, so a failed write keeps the old file intact.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no file name")
    })?;

    let (temp_path, mut file) = loop {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            id
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => break (temp_path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };

    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_and_preserves_target_on_replace_failure() {
        let root = std::env::temp_dir().join(format!("flowscale-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("settings.json");
        fs::write(&target, b"old").unwrap();
        write_atomic(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let locked=OpenOptions::new().read(true).share_mode(1).open(&target).unwrap();
            assert!(write_atomic(&target,b"must not replace locked file").is_err());
            assert_eq!(fs::read(&target).unwrap(),b"new");
            drop(locked);
        }

        let blocker = root.join("blocked");
        fs::create_dir(&blocker).unwrap();
        fs::write(blocker.join("keep"), b"old bytes").unwrap();
        assert!(write_atomic(&blocker, b"replacement").is_err());
        assert_eq!(fs::read(blocker.join("keep")).unwrap(), b"old bytes");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        let _ = fs::remove_dir_all(root);
    }
}
