use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::config::WorkspaceConfig;
use crate::error::TexoError;

use super::{
    BackupFinding, FileRecord, CONFIG_FILE, MAX_CONFIG_BYTES, MAX_STORE_FILES, MAX_STORE_FILE_BYTES,
};

pub(super) fn digest_from_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, slot) in digest.iter_mut().enumerate() {
        let offset = index * 2;
        *slot = u8::from_str_radix(&value[offset..offset + 2], 16).ok()?;
    }
    Some(digest)
}

pub(super) fn hash_store_files(directory: &Path) -> Result<Vec<FileRecord>, TexoError> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(backup_error(format!(
                "snapshot entry `{}` is not a regular file",
                entry.path().display()
            )));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| backup_error("snapshot file name is not UTF-8"))?;
        if records.len() >= MAX_STORE_FILES {
            return Err(backup_error(format!(
                "snapshot exceeds the {MAX_STORE_FILES}-file limit"
            )));
        }
        let (hash_hex, bytes) =
            hash_regular_bounded(&entry.path(), MAX_STORE_FILE_BYTES).map_err(backup_error)?;
        records.push(FileRecord {
            name,
            bytes,
            hash_hex,
        });
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(records)
}

pub(super) fn copy_config(root: &Path, dest: &Path) -> Result<(String, u64), TexoError> {
    let source = root.join(".texo/config.toml");
    let bytes = read_regular_bounded(&source, MAX_CONFIG_BYTES).map_err(backup_error)?;
    write_new_synced(&dest.join(CONFIG_FILE), &bytes)?;
    Ok((
        blake3::hash(&bytes).to_hex().to_string(),
        bytes.len() as u64,
    ))
}

pub(super) fn read_regular_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds the {limit}-byte limit", path.display()));
    }
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn hash_regular_bounded(path: &Path, limit: u64) -> Result<(String, u64), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(format!("{} is not a bounded regular file", path.display()));
    }
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        if bytes > limit {
            return Err(format!(
                "{} grew beyond the {limit}-byte limit",
                path.display()
            ));
        }
        hasher.update(&buffer[..count]);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

pub(super) fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), TexoError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn reject_overlap(
    root: &Path,
    workspace: &WorkspaceConfig,
    dest: &Path,
) -> Result<(), TexoError> {
    let root = absolute_path(root)?;
    let texo = absolute_path(&root.join(".texo"))?;
    let store = absolute_path(&workspace.store_path_buf(&root))?;
    for live in [&root, &texo, &store] {
        if dest.starts_with(live) || live.starts_with(dest) {
            return Err(backup_error(
                "backup destination must not overlap the workspace root, .texo, or live store",
            ));
        }
    }
    Ok(())
}

pub(super) fn safe_flat_name(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name)
}

pub(super) fn absolute_path(path: &Path) -> Result<PathBuf, TexoError> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(backup_error("path escapes the filesystem root"));
                }
            }
            kept @ (Component::Prefix(_) | Component::RootDir | Component::Normal(_)) => {
                normalized.push(kept);
            }
        }
    }
    let mut existing = normalized.clone();
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = fs::canonicalize(&existing)?;
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or_else(|| backup_error("path has no existing ancestor"))?;
                missing.push(name.to_os_string());
                if !existing.pop() {
                    return Err(backup_error("path has no existing ancestor"));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) fn sync_directory(path: &Path) -> Result<(), TexoError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(super) fn finding(kind: &'static str, detail: impl Into<String>) -> BackupFinding {
    BackupFinding {
        kind,
        detail: detail.into(),
    }
}

pub(super) fn backup_error(detail: impl Into<String>) -> TexoError {
    TexoError::Backup {
        detail: detail.into(),
    }
}

pub(super) fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut encoded, byte| {
            let _result = write!(encoded, "{byte:02x}");
            encoded
        },
    )
}
