//! WebDAV cloud sync for the database snapshot.
//!
//! One remote file (`<remote_dir>/fluxgate.db`) mirrors the local database.
//! Upload writes a consistent `VACUUM INTO` snapshot; download stages the
//! remote file via `storage::stage_restore_bytes`, so the swap happens on the
//! next launch and never corrupts the live connection.

use crate::{db::Database, models::WebdavConfig, storage};
use anyhow::{bail, Context, Result};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub remote_file: String,
    pub size_bytes: u64,
}

fn validate(config: &WebdavConfig) -> Result<()> {
    let url = config.url.trim();
    if url.is_empty() {
        bail!("请先填写 WebDAV 服务器地址");
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("WebDAV 地址必须以 http:// 或 https:// 开头");
    }
    Ok(())
}

/// `base` + optional `dir` segments + optional file, with single slashes.
fn join_url(base: &str, dir: &str, file: Option<&str>) -> String {
    let mut url = base.trim().trim_end_matches('/').to_string();
    for segment in dir.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        url.push('/');
        url.push_str(segment);
    }
    if let Some(file) = file {
        url.push('/');
        url.push_str(file);
    }
    url
}

fn authed(builder: RequestBuilder, config: &WebdavConfig) -> RequestBuilder {
    if config.username.trim().is_empty() {
        builder
    } else {
        builder.basic_auth(config.username.trim(), Some(config.password.as_str()))
    }
}

/// Cheap connectivity + credential check against the server base URL.
pub async fn test(http: &reqwest::Client, config: &WebdavConfig) -> Result<()> {
    validate(config)?;
    let url = join_url(&config.url, "", None);
    let response = authed(
        http.request(Method::from_bytes(b"PROPFIND").expect("method"), &url),
        config,
    )
    .header("Depth", "0")
    .send()
    .await
    .context("无法连接 WebDAV 服务器")?;
    match response.status() {
        StatusCode::UNAUTHORIZED => bail!("认证失败：用户名或密码错误"),
        status if status.is_client_error() && status != StatusCode::METHOD_NOT_ALLOWED => {
            bail!("服务器拒绝请求（HTTP {}）", status.as_u16())
        }
        status if status.is_server_error() => bail!("服务器错误（HTTP {}）", status.as_u16()),
        _ => Ok(()),
    }
}

/// Create the remote directory chain; servers answer 405 for existing dirs.
async fn ensure_remote_dir(http: &reqwest::Client, config: &WebdavConfig) -> Result<()> {
    let mut path = String::new();
    for segment in config
        .remote_dir
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        path = if path.is_empty() {
            segment.to_string()
        } else {
            format!("{path}/{segment}")
        };
        let url = join_url(&config.url, &path, None);
        let response = authed(
            http.request(Method::from_bytes(b"MKCOL").expect("method"), &url),
            config,
        )
        .send()
        .await?;
        let status = response.status();
        if !status.is_success()
            && status != StatusCode::METHOD_NOT_ALLOWED
            && status != StatusCode::CONFLICT
        {
            bail!("创建远程目录 {path} 失败（HTTP {}）", status.as_u16());
        }
    }
    Ok(())
}

/// Snapshot the live database and upload it as the remote file.
pub async fn upload(
    http: &reqwest::Client,
    config: &WebdavConfig,
    db: &Database,
    data_dir: &Path,
) -> Result<SyncReport> {
    validate(config)?;
    let staging = data_dir.join(format!("webdav-upload-{}.db", uuid::Uuid::new_v4()));
    let result = upload_inner(http, config, db, &staging).await;
    let _ = std::fs::remove_file(&staging);
    result
}

async fn upload_inner(
    http: &reqwest::Client,
    config: &WebdavConfig,
    db: &Database,
    staging: &Path,
) -> Result<SyncReport> {
    db.snapshot_to(staging)?;
    let bytes = std::fs::read(staging).context("read upload snapshot")?;
    let size_bytes = bytes.len() as u64;
    ensure_remote_dir(http, config).await?;
    let url = join_url(&config.url, &config.remote_dir, Some(storage::DB_FILE));
    let response = authed(http.put(&url), config)
        .header("content-type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .context("上传失败：无法连接服务器")?;
    let status = response.status();
    if !status.is_success() {
        bail!("上传失败（HTTP {}）", status.as_u16());
    }
    Ok(SyncReport {
        remote_file: url,
        size_bytes,
    })
}

/// Download the remote snapshot and stage it for the next launch.
pub async fn download(
    http: &reqwest::Client,
    config: &WebdavConfig,
    data_dir: &Path,
) -> Result<SyncReport> {
    validate(config)?;
    let url = join_url(&config.url, &config.remote_dir, Some(storage::DB_FILE));
    let response = authed(http.get(&url), config)
        .send()
        .await
        .context("下载失败：无法连接服务器")?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        bail!("云端还没有备份，请先在另一台设备上执行上传");
    }
    if !status.is_success() {
        bail!("下载失败（HTTP {}）", status.as_u16());
    }
    let bytes = response.bytes().await.context("读取下载内容失败")?;
    storage::stage_restore_bytes(data_dir, &bytes)?;
    Ok(SyncReport {
        remote_file: url,
        size_bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::GatewayConfig;
    use axum::{extract::State as AxumState, http::StatusCode as AxStatus, Router};
    use std::sync::{Arc, Mutex};

    #[test]
    fn joins_urls_without_duplicate_slashes() {
        assert_eq!(
            join_url("https://dav.example.com/dav/", "fluxgate/main", Some("fluxgate.db")),
            "https://dav.example.com/dav/fluxgate/main/fluxgate.db"
        );
        assert_eq!(
            join_url("https://dav.example.com", " / fluxgate / ", None),
            "https://dav.example.com/fluxgate"
        );
        assert_eq!(join_url("https://dav.example.com/", "", None), "https://dav.example.com");
    }

    #[test]
    fn validates_config_urls() {
        let mut config = WebdavConfig::default();
        assert!(validate(&config).is_err());
        config.url = "ftp://x".into();
        assert!(validate(&config).is_err());
        config.url = "https://dav.example.com".into();
        assert!(validate(&config).is_ok());
    }

    #[tokio::test]
    async fn uploads_and_downloads_snapshot_roundtrip() {
        type Store = Arc<Mutex<Option<Vec<u8>>>>;
        let store: Store = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route(
                "/dav/fluxgate/fluxgate.db",
                axum::routing::put(
                    |AxumState(store): AxumState<Store>, body: axum::body::Bytes| async move {
                        *store.lock().unwrap() = Some(body.to_vec());
                        AxStatus::CREATED
                    },
                )
                .get(|AxumState(store): AxumState<Store>| async move {
                    store.lock().unwrap().clone().map_or_else(
                        || (AxStatus::NOT_FOUND, Vec::new()),
                        |bytes| (AxStatus::OK, bytes),
                    )
                }),
            )
            .fallback(|request: axum::extract::Request| async move {
                // MKCOL / PROPFIND land here; accept them.
                let method = request.method().as_str().to_string();
                if method == "MKCOL" || method == "PROPFIND" {
                    AxStatus::CREATED
                } else {
                    AxStatus::METHOD_NOT_ALLOWED
                }
            })
            .with_state(store.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let data_dir =
            std::env::temp_dir().join(format!("fluxgate-webdav-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let db = Database::open(&data_dir.join(storage::DB_FILE)).unwrap();
        db.save_config(&GatewayConfig::default()).unwrap();
        let config = WebdavConfig {
            url: format!("http://{address}/dav"),
            username: "user".into(),
            password: "pass".into(),
            remote_dir: "fluxgate".into(),
        };
        let http = reqwest::Client::new();

        test(&http, &config).await.unwrap();
        let uploaded = upload(&http, &config, &db, &data_dir).await.unwrap();
        assert!(uploaded.size_bytes > 0);
        let downloaded = download(&http, &config, &data_dir).await.unwrap();
        assert_eq!(downloaded.size_bytes, uploaded.size_bytes);
        assert!(data_dir.join("restore-pending.db").exists());
        server.abort();
    }
}
