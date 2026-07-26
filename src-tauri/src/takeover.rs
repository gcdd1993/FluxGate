use crate::models::GatewayConfig;
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut};

pub fn apply_changes(previous: &GatewayConfig, next: &GatewayConfig) -> Result<()> {
    let endpoint = format!("http://{}:{}", next.host, next.port);
    if previous.clients.claude != next.clients.claude {
        apply_claude(next.clients.claude, &endpoint)?;
    }
    if previous.clients.codex != next.clients.codex {
        apply_codex(next.clients.codex, &endpoint)?;
    }
    Ok(())
}

fn home_file(parts: &[&str]) -> Result<PathBuf> {
    let mut path = dirs::home_dir().context("home directory is unavailable")?;
    for part in parts {
        path.push(part)
    }
    Ok(path)
}
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".fluxgate.bak");
    PathBuf::from(name)
}
fn ensure_backup(path: &Path) -> Result<()> {
    let backup = backup_path(path);
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup).with_context(|| format!("backup {}", path.display()))?;
    }
    Ok(())
}
fn restore(path: &Path) -> Result<()> {
    let backup = backup_path(path);
    if backup.exists() {
        fs::copy(&backup, path).with_context(|| format!("restore {}", path.display()))?;
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn apply_claude(enabled: bool, endpoint: &str) -> Result<()> {
    let path = home_file(&[".claude", "settings.json"])?;
    if !enabled {
        return restore(&path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?
    }
    ensure_backup(&path)?;
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?).context("parse Claude settings.json")?
    } else {
        Value::Object(Map::new())
    };
    let obj = root
        .as_object_mut()
        .context("Claude settings root must be an object")?;
    let env = obj
        .entry("env")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("Claude env must be an object")?;
    env.insert("ANTHROPIC_BASE_URL".into(), Value::String(endpoint.into()));
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String("fluxgate-local".into()),
    );
    fs::write(&path, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn apply_codex(enabled: bool, endpoint: &str) -> Result<()> {
    let path = home_file(&[".codex", "config.toml"])?;
    if !enabled {
        return restore(&path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?
    }
    ensure_backup(&path)?;
    let source = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut doc = source
        .parse::<DocumentMut>()
        .context("parse Codex config.toml")?;
    doc["model_provider"] = value("fluxgate");
    doc["model_providers"]["fluxgate"]["name"] = value("FluxGate Local");
    doc["model_providers"]["fluxgate"]["base_url"] = value(format!("{endpoint}/v1"));
    doc["model_providers"]["fluxgate"]["env_key"] = value("FLUXGATE_API_KEY");
    doc["model_providers"]["fluxgate"]["wire_api"] = value("responses");
    fs::write(&path, doc.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
