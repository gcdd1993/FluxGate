//! Data-directory redirection plus database backup/restore.
//!
//! The SQLite file normally lives in the OS app-data directory. A small
//! pointer file (`data-dir-override.txt`) placed there can redirect the whole
//! data directory to a user-chosen location (cloud drive, NAS, ...). Restores
//! never touch the live database: they stage the snapshot as
//! `restore-pending.db` and the swap happens on the next launch, before any
//! connection is opened.

use crate::db::Database;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const DB_FILE: &str = "fluxgate.db";
const OVERRIDE_FILE: &str = "data-dir-override.txt";
const PENDING_FILE: &str = "restore-pending.db";
const BACKUP_DIR: &str = "backups";
/// Rotation cap for the backups directory.
const MAX_BACKUPS: usize = 10;
/// Startup auto-backups are skipped while the newest backup is younger.
const AUTO_BACKUP_MIN_AGE_HOURS: i64 = 6;

/// Paths the storage commands operate on, resolved once at startup.
pub struct StoragePaths {
    /// OS default app-data directory; always hosts the override pointer.
    pub default_dir: PathBuf,
    /// Effective data directory after applying the override.
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub data_dir: String,
    pub default_dir: String,
    pub is_custom: bool,
    pub db_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub file_name: String,
    pub size_bytes: u64,
    pub created_ms: i64,
}

/// Resolve the effective data directory: the override pointer wins when it
/// names a usable directory, otherwise the OS default applies.
pub fn resolve(default_dir: PathBuf) -> StoragePaths {
    let pointer = default_dir.join(OVERRIDE_FILE);
    let custom = fs::read_to_string(&pointer)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && fs::create_dir_all(path).is_ok());
    StoragePaths {
        data_dir: custom.unwrap_or_else(|| default_dir.clone()),
        default_dir,
    }
}

/// Point the data directory at `target` and seed it with a snapshot of the
/// current database when it does not hold one yet. Takes effect on restart.
pub fn set_data_dir(paths: &StoragePaths, db: &Database, target: &str) -> Result<()> {
    let target = target.trim();
    if target.is_empty() {
        bail!("目录不能为空");
    }
    let target_path = PathBuf::from(target);
    if !target_path.is_absolute() {
        bail!("请输入绝对路径");
    }
    fs::create_dir_all(&target_path)
        .with_context(|| format!("无法创建目录 {}", target_path.display()))?;
    if target_path == paths.data_dir {
        bail!("已经在使用该目录");
    }
    let target_db = target_path.join(DB_FILE);
    if !target_db.exists() {
        db.snapshot_to(&target_db)?;
    }
    fs::create_dir_all(&paths.default_dir)?;
    fs::write(paths.default_dir.join(OVERRIDE_FILE), target)
        .context("write data-dir override pointer")?;
    Ok(())
}

/// Drop the override pointer so the OS default directory applies again.
/// The current database is snapshotted back when the default has none.
pub fn reset_data_dir(paths: &StoragePaths, db: &Database) -> Result<()> {
    let default_db = paths.default_dir.join(DB_FILE);
    if paths.data_dir != paths.default_dir && !default_db.exists() {
        db.snapshot_to(&default_db)?;
    }
    let pointer = paths.default_dir.join(OVERRIDE_FILE);
    if pointer.exists() {
        fs::remove_file(pointer).context("remove data-dir override pointer")?;
    }
    Ok(())
}

pub fn info(paths: &StoragePaths) -> StorageInfo {
    StorageInfo {
        data_dir: paths.data_dir.display().to_string(),
        default_dir: paths.default_dir.display().to_string(),
        is_custom: paths.data_dir != paths.default_dir,
        db_size_bytes: fs::metadata(paths.data_dir.join(DB_FILE))
            .map(|meta| meta.len())
            .unwrap_or(0),
    }
}

/// Swap in a staged restore before any database connection exists. The
/// previous database is preserved as a `pre-restore` backup and stale WAL
/// sidecars are dropped so SQLite cannot replay old pages over the snapshot.
pub fn apply_pending_restore(data_dir: &Path) -> Result<()> {
    let pending = data_dir.join(PENDING_FILE);
    if !pending.exists() {
        return Ok(());
    }
    let db = data_dir.join(DB_FILE);
    if db.exists() {
        let safety = data_dir
            .join(BACKUP_DIR)
            .join(format!("fluxgate-pre-restore-{}.db", timestamp()));
        if let Some(parent) = safety.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&db, &safety).context("snapshot current database before restore")?;
    }
    for sidecar in ["-wal", "-shm"] {
        let _ = fs::remove_file(data_dir.join(format!("{DB_FILE}{sidecar}")));
    }
    let _ = fs::remove_file(&db);
    fs::rename(&pending, &db).context("activate staged restore")?;
    Ok(())
}

/// Stage `bytes` as the next database. Content must be a SQLite file.
pub fn stage_restore_bytes(data_dir: &Path, bytes: &[u8]) -> Result<()> {
    if !bytes.starts_with(b"SQLite format 3\0") {
        bail!("下载内容不是有效的 SQLite 数据库");
    }
    fs::create_dir_all(data_dir)?;
    fs::write(data_dir.join(PENDING_FILE), bytes).context("stage restore file")?;
    Ok(())
}

/// Stage an existing local backup as the next database.
pub fn stage_restore_backup(data_dir: &Path, file_name: &str) -> Result<()> {
    let source = data_dir.join(BACKUP_DIR).join(sanitize(file_name)?);
    let bytes = fs::read(&source).with_context(|| format!("读取备份 {file_name}"))?;
    stage_restore_bytes(data_dir, &bytes)
}

/// Create a consistent snapshot in the backups directory and rotate old ones.
pub fn create_backup(data_dir: &Path, db: &Database) -> Result<BackupEntry> {
    let file_name = format!("fluxgate-{}.db", timestamp());
    let target = data_dir.join(BACKUP_DIR).join(&file_name);
    db.snapshot_to(&target)?;
    rotate(data_dir)?;
    let meta = fs::metadata(&target)?;
    Ok(BackupEntry {
        file_name,
        size_bytes: meta.len(),
        created_ms: chrono::Local::now().timestamp_millis(),
    })
}

/// Startup hook: keep a recent automatic backup without churning one out on
/// every launch. Never fails the boot path.
pub fn auto_backup(data_dir: &Path, db: &Database) {
    let fresh_enough = list_backups(data_dir).first().is_some_and(|newest| {
        chrono::Local::now().timestamp_millis() - newest.created_ms
            < AUTO_BACKUP_MIN_AGE_HOURS * 3_600_000
    });
    if fresh_enough {
        return;
    }
    if let Err(error) = create_backup(data_dir, db) {
        eprintln!("FluxGate auto backup failed: {error:#}");
    }
}

/// Newest-first listing of the backups directory.
pub fn list_backups(data_dir: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = fs::read_dir(data_dir.join(BACKUP_DIR)) else {
        return Vec::new();
    };
    let mut backups = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if !name.starts_with("fluxgate-") || !name.ends_with(".db") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let created = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some(BackupEntry {
                file_name: name,
                size_bytes: meta.len(),
                created_ms: created.as_millis() as i64,
            })
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| std::cmp::Reverse(entry.created_ms));
    backups
}

pub fn delete_backup(data_dir: &Path, file_name: &str) -> Result<()> {
    let path = data_dir.join(BACKUP_DIR).join(sanitize(file_name)?);
    fs::remove_file(path).with_context(|| format!("删除备份 {file_name}"))
}

fn rotate(data_dir: &Path) -> Result<()> {
    for stale in list_backups(data_dir).into_iter().skip(MAX_BACKUPS) {
        let _ = fs::remove_file(data_dir.join(BACKUP_DIR).join(stale.file_name));
    }
    Ok(())
}

/// Backup names come from the frontend; reject anything that could escape
/// the backups directory.
fn sanitize(file_name: &str) -> Result<&str> {
    let valid = file_name.starts_with("fluxgate-")
        && file_name.ends_with(".db")
        && !file_name.contains(['/', '\\'])
        && !file_name.contains("..");
    if !valid {
        bail!("非法的备份文件名");
    }
    Ok(file_name)
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fluxgate-storage-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sanitize_rejects_path_traversal() {
        assert!(sanitize("fluxgate-20260726-010203.db").is_ok());
        assert!(sanitize("../fluxgate-x.db").is_err());
        assert!(sanitize("fluxgate-..\\x.db").is_err());
        assert!(sanitize("other.db").is_err());
        assert!(sanitize("fluxgate-x.txt").is_err());
    }

    #[test]
    fn resolve_prefers_valid_override() {
        let default_dir = temp_dir("default");
        let custom = temp_dir("custom");
        fs::write(
            default_dir.join(OVERRIDE_FILE),
            custom.display().to_string(),
        )
        .unwrap();
        let paths = resolve(default_dir.clone());
        assert_eq!(paths.data_dir, custom);
        assert!(info(&paths).is_custom);

        // Relative or empty pointers fall back to the default directory.
        fs::write(default_dir.join(OVERRIDE_FILE), "not/absolute").unwrap();
        assert_eq!(resolve(default_dir.clone()).data_dir, default_dir);
    }

    #[test]
    fn backup_create_list_rotate_and_restore_cycle() {
        let data_dir = temp_dir("cycle");
        let db = Database::open(&data_dir.join(DB_FILE)).unwrap();
        db.save_config(&crate::models::GatewayConfig::default())
            .unwrap();

        let entry = create_backup(&data_dir, &db).unwrap();
        assert!(entry.size_bytes > 0);
        assert_eq!(list_backups(&data_dir).len(), 1);

        // Stage the backup and swap it in like the next launch would.
        stage_restore_backup(&data_dir, &entry.file_name).unwrap();
        drop(db);
        apply_pending_restore(&data_dir).unwrap();
        assert!(!data_dir.join(PENDING_FILE).exists());
        // The pre-restore safety snapshot joins the original backup.
        assert_eq!(list_backups(&data_dir).len(), 2);
        // The restored database still opens and keeps its settings.
        let restored = Database::open(&data_dir.join(DB_FILE)).unwrap();
        assert_eq!(
            restored.config().port,
            crate::models::GatewayConfig::default().port
        );
    }

    #[test]
    fn stage_restore_rejects_non_sqlite_payload() {
        let data_dir = temp_dir("reject");
        assert!(stage_restore_bytes(&data_dir, b"<html>login page</html>").is_err());
    }
}
