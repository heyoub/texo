//! Managed-path validation and crash-safe file mutation primitives.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::error::TexoError;

use super::{config_error, ChangeAction, InstallChange, AGENT_GUIDE_PATH};

static INSTALL_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn remove_managed_file(
    root: &Path,
    relative: &str,
    schema: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    let action = if path.is_file() {
        let document = serde_json::from_slice::<Value>(&std::fs::read(&path)?)?;
        if document.get("schema").and_then(Value::as_str) != Some(schema) {
            return Err(config_error(
                relative,
                "file is not managed by this installer",
            ));
        }
        if !dry_run {
            std::fs::remove_file(path)?;
        }
        ChangeAction::Removed
    } else {
        ChangeAction::Unchanged
    };
    Ok(InstallChange {
        path: relative.to_string(),
        action,
    })
}

pub(super) fn remove_marked_block(
    root: &Path,
    relative: &str,
    start: &str,
    end: &str,
    remove_empty: bool,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    let existing = read_optional_string(&path)?;
    let (without, had_marker) = strip_marked_block(&existing, start, end)?;
    if !had_marker {
        return Ok(None);
    }
    if !dry_run {
        if remove_empty && without.trim().is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            atomic_write(&path, without.as_bytes())?;
        }
    }
    Ok(Some(InstallChange {
        path: relative.to_string(),
        action: ChangeAction::Removed,
    }))
}

pub(super) fn write_managed(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    let existed = path.exists();
    let action = classify_bytes(&path, bytes)?;
    if !dry_run && action != ChangeAction::Unchanged {
        atomic_write(&path, bytes)?;
    }
    Ok(InstallChange {
        path: relative.to_string(),
        action: if existed {
            action
        } else {
            ChangeAction::Created
        },
    })
}

pub(super) fn classify_bytes(path: &Path, wanted: &[u8]) -> Result<ChangeAction, TexoError> {
    match std::fs::read(path) {
        Ok(existing) if existing == wanted => Ok(ChangeAction::Unchanged),
        Ok(_) => Ok(ChangeAction::Updated),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ChangeAction::Created),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn ensure_safe_managed_path(root: &Path, relative: &str) -> Result<(), TexoError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(config_error(
            relative,
            "managed path must remain below the workspace root",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        if let std::path::Component::Normal(name) = component {
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(config_error(
                        relative,
                        &format!("managed path crosses symbolic link `{}`", current.display()),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TexoError> {
    let parent = path
        .parent()
        .ok_or_else(|| config_error(&path.display().to_string(), "managed path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let existing_permissions = std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .map(|metadata| metadata.permissions());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| config_error(&path.display().to_string(), "file name is not UTF-8"))?;
    for _attempt in 0..100 {
        let counter = INSTALL_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(
            ".{name}.texo-install-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> std::io::Result<()> {
            file.write_all(bytes)?;
            if let Some(permissions) = &existing_permissions {
                file.set_permissions(permissions.clone())?;
            }
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _removed = std::fs::remove_file(&tmp);
        }
        return result.map_err(Into::into);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private install staging file",
    )
    .into())
}

pub(super) fn strip_marked_block(
    input: &str,
    start: &str,
    end: &str,
) -> Result<(String, bool), TexoError> {
    let Some(start_offset) = input.find(start) else {
        if input.contains(end) {
            return Err(config_error(
                AGENT_GUIDE_PATH,
                "orphaned managed end marker",
            ));
        }
        return Ok((input.to_string(), false));
    };
    let Some(relative_end) = input[start_offset..].find(end) else {
        return Err(config_error(
            AGENT_GUIDE_PATH,
            "unterminated managed marker block",
        ));
    };
    let end_offset = start_offset + relative_end + end.len();
    if input[end_offset..].contains(start) {
        return Err(config_error(
            AGENT_GUIDE_PATH,
            "multiple managed marker blocks",
        ));
    }
    let mut without = String::new();
    without.push_str(input[..start_offset].trim_end());
    if !without.is_empty() {
        without.push('\n');
    }
    without.push_str(input[end_offset..].trim_start_matches(['\r', '\n']));
    Ok((without, true))
}

pub(super) fn append_block(existing: &str, block: &str) -> String {
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out.push('\n');
    out
}

pub(super) fn read_optional_string(path: &Path) -> Result<String, TexoError> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn with_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

pub(super) fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
