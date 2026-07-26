use crate::models::{BalanceInfo, Provider};
use anyhow::{bail, Context, Result};
use serde_json::Value;

/// NewAPI counts quota in integer units of 1/500000 USD.
const NEWAPI_QUOTA_PER_USD: f64 = 500_000.0;

/// Query the remaining account balance of a provider console. Dispatch is
/// keyed by `Provider::balance.kind`; supporting a new console means adding
/// one match arm plus its fetch function below — nothing else changes.
pub async fn fetch(http: &reqwest::Client, provider: &Provider) -> Result<BalanceInfo> {
    match provider.balance.kind.as_str() {
        "newapi" => fetch_newapi(http, provider).await,
        "" => bail!("该供应商未启用余额查询"),
        other => bail!("未知的余额查询类型：{other}"),
    }
}

/// NewAPI-style consoles (new-api / one-api forks) expose `GET /api/user/self`
/// authenticated with a personal access token plus the `New-Api-User` id —
/// the same protocol cc-switch's NewAPI usage preset speaks. `data.quota` is
/// the remaining balance and `data.used_quota` the consumed amount.
async fn fetch_newapi(http: &reqwest::Client, provider: &Provider) -> Result<BalanceInfo> {
    let query = &provider.balance;
    if query.token.trim().is_empty() || query.user_id.trim().is_empty() {
        bail!("请先填写控制台访问令牌与用户 ID（在 NewAPI 个人设置 → 安全设置中生成）");
    }
    let root = if query.base_url.trim().is_empty() {
        api_root(&provider.base_url)
    } else {
        query.base_url.trim().trim_end_matches('/').to_string()
    };
    let response = http
        .get(format!("{root}/api/user/self"))
        .bearer_auth(query.token.trim())
        .header("New-Api-User", query.user_id.trim())
        .header("accept", "application/json")
        .send()
        .await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "余额查询失败（HTTP {}）：{}",
            status.as_u16(),
            String::from_utf8_lossy(&body)
                .chars()
                .take(300)
                .collect::<String>()
        );
    }
    let value: Value = serde_json::from_slice(&body).context("余额响应不是合法 JSON")?;
    if !value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("控制台返回 success=false");
        bail!("余额查询失败：{message}");
    }
    let data = value.get("data").context("余额响应缺少 data 字段")?;
    let remaining =
        data.get("quota").and_then(Value::as_f64).unwrap_or(0.0) / NEWAPI_QUOTA_PER_USD;
    let used = data
        .get("used_quota")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        / NEWAPI_QUOTA_PER_USD;
    Ok(BalanceInfo {
        total: remaining + used,
        used,
        remaining,
        currency: "USD".into(),
    })
}

/// Strip a trailing `/v1` so both `https://host` and `https://host/v1`
/// style base URLs resolve to the console root.
fn api_root(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BalanceQuery;
    use axum::{http::HeaderMap, routing::get, Json, Router};
    use serde_json::json;

    fn provider(base_url: String, balance: BalanceQuery) -> Provider {
        Provider {
            id: "p1".into(),
            name: "p1".into(),
            protocol: "openai".into(),
            base_url,
            api_key: "sk-relay".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
            balance,
        }
    }

    fn newapi_query() -> BalanceQuery {
        BalanceQuery {
            kind: "newapi".into(),
            base_url: String::new(),
            token: "console-token".into(),
            user_id: "114514".into(),
        }
    }

    #[test]
    fn api_root_strips_v1_suffix() {
        assert_eq!(api_root("https://x.me/v1"), "https://x.me");
        assert_eq!(api_root("https://x.me/v1/"), "https://x.me");
        assert_eq!(api_root("https://x.me"), "https://x.me");
    }

    #[tokio::test]
    async fn fetches_newapi_balance_with_console_token() {
        let app = Router::new().route(
            "/api/user/self",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers.get("authorization").unwrap(),
                    "Bearer console-token"
                );
                assert_eq!(headers.get("new-api-user").unwrap(), "114514");
                Json(json!({
                    "success": true,
                    "data": { "quota": 47_300_000.0, "used_quota": 12_700_000.0, "group": "default" }
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // The relay base URL carries /v1; the console root must be derived.
        let provider = provider(format!("http://{address}/v1"), newapi_query());
        let info = fetch(&reqwest::Client::new(), &provider).await.unwrap();
        assert_eq!(info.remaining, 94.6);
        assert_eq!(info.used, 25.4);
        assert_eq!(info.total, 120.0);
        assert_eq!(info.currency, "USD");
        server.abort();
    }

    #[tokio::test]
    async fn surfaces_console_error_message() {
        let app = Router::new().route(
            "/api/user/self",
            get(|| async { Json(json!({"success": false, "message": "无权进行此操作，未登录且未提供 access token"})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = provider(format!("http://{address}/v1"), newapi_query());
        let error = fetch(&reqwest::Client::new(), &provider)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("无权进行此操作"));
        server.abort();
    }

    #[tokio::test]
    async fn requires_console_credentials() {
        let provider = provider(
            "https://x.me/v1".into(),
            BalanceQuery {
                kind: "newapi".into(),
                ..Default::default()
            },
        );
        let error = fetch(&reqwest::Client::new(), &provider)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("访问令牌"));
    }

    #[tokio::test]
    async fn rejects_disabled_balance_query() {
        let provider = provider("https://x.me/v1".into(), BalanceQuery::default());
        let error = fetch(&reqwest::Client::new(), &provider)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("未启用余额查询"));
    }
}
