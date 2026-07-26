use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Codex,
    Claude,
    Openai,
}

impl ClientKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Openai => "openai",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
    pub client: String,
    pub health: String,
    pub failures: i64,
    #[serde(default)]
    pub models: Vec<String>,
    /// Optional provider-level User-Agent override. Empty values preserve the
    /// client User-Agent, matching cc-switch's default routing behavior.
    #[serde(default)]
    pub custom_user_agent: Option<String>,
    /// Console balance-query settings; `kind` empty means disabled.
    #[serde(default)]
    pub balance: BalanceQuery,
}

/// How to query the remaining balance on a provider's console. Modeled after
/// cc-switch's usage-script presets: NewAPI-style consoles authenticate with a
/// personal access token + user id rather than the relay API key.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BalanceQuery {
    /// "" (disabled) or "newapi"; new kinds plug into balance.rs.
    #[serde(default)]
    pub kind: String,
    /// Console root override. Empty derives it from the provider base URL.
    #[serde(default)]
    pub base_url: String,
    /// Access token generated in the console's personal security settings.
    #[serde(default)]
    pub token: String,
    /// Console user id, sent as the `New-Api-User` header.
    #[serde(default)]
    pub user_id: String,
}

/// Snapshot of an upstream account balance returned by `balance::fetch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BalanceInfo {
    pub total: f64,
    pub used: f64,
    pub remaining: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAlias {
    pub id: String,
    pub alias: String,
    #[serde(default)]
    pub targets: Vec<AliasTarget>,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub upstream_model: String,
    pub client: String,
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub failover_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasTarget {
    pub id: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl ModelAlias {
    pub fn normalize(&mut self) {
        if self.targets.is_empty()
            && !self.provider_id.is_empty()
            && !self.upstream_model.is_empty()
        {
            self.targets.push(AliasTarget {
                id: format!("legacy-{}", self.id),
                provider_id: self.provider_id.clone(),
                upstream_model: self.upstream_model.clone(),
                enabled: true,
            });
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientFlags {
    pub codex: bool,
    pub claude: bool,
    pub openai: bool,
}
impl ClientFlags {
    pub fn enabled(&self, c: &ClientKind) -> bool {
        match c {
            ClientKind::Codex => self.codex,
            ClientKind::Claude => self.claude,
            ClientKind::Openai => self.openai,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub record_usage: bool,
    pub outbound_proxy: String,
    pub clients: ClientFlags,
    pub auto_failover: ClientFlags,
    pub failure_threshold: u32,
    pub recovery_seconds: u64,
    pub max_retries: u32,
    #[serde(default)]
    pub rectifier: RectifierConfig,
    /// Inject `cache_control` breakpoints into Anthropic-bound requests so
    /// clients that never mark prompts still hit Anthropic's prompt cache.
    #[serde(default = "default_true")]
    pub cache_injection: bool,
    /// Hide to the system tray instead of exiting when the window closes.
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    /// Start FluxGate automatically at login.
    #[serde(default)]
    pub launch_at_login: bool,
    /// WebDAV cloud-sync endpoint for the database snapshot.
    #[serde(default)]
    pub webdav: WebdavConfig,
}

/// Connection settings for syncing the database snapshot over WebDAV.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebdavConfig {
    /// Server base URL, e.g. `https://dav.jianguoyun.com/dav`.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// Remote folder (may be nested, e.g. `fluxgate/main`).
    #[serde(default = "default_webdav_dir")]
    pub remote_dir: String,
}

fn default_webdav_dir() -> String {
    "fluxgate".into()
}
impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 18080,
            record_usage: true,
            outbound_proxy: String::new(),
            clients: ClientFlags {
                codex: false,
                claude: false,
                openai: true,
            },
            auto_failover: ClientFlags {
                codex: true,
                claude: true,
                openai: false,
            },
            failure_threshold: 4,
            recovery_seconds: 60,
            max_retries: 3,
            rectifier: RectifierConfig::default(),
            cache_injection: true,
            minimize_to_tray: true,
            launch_at_login: false,
            webdav: WebdavConfig {
                remote_dir: default_webdav_dir(),
                ..WebdavConfig::default()
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RectifierConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub thinking_signature: bool,
    #[serde(default = "default_true")]
    pub thinking_budget: bool,
    #[serde(default = "default_true")]
    pub media_fallback: bool,
    #[serde(default = "default_true")]
    pub text_only_preflight: bool,
}

impl Default for RectifierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thinking_signature: true,
            thinking_budget: true,
            media_fallback: true,
            text_only_preflight: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageFilter {
    pub range: String,
    pub client: String,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLog {
    pub id: i64,
    pub timestamp: String,
    pub client: String,
    pub provider_name: String,
    pub provider_id: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cost: f64,
    pub latency_ms: i64,
    pub status: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsagePoint {
    pub label: String,
    pub input: i64,
    pub output: i64,
    pub cached: i64,
    pub cost: f64,
    pub requests: i64,
}
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub requests: i64,
    pub tokens: i64,
    pub cache_hit_rate: f64,
    pub cost: f64,
    pub success_rate: f64,
    pub avg_latency_ms: i64,
}
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GroupStat {
    pub name: String,
    pub requests: i64,
    pub tokens: i64,
    pub success_rate: f64,
    pub cost: f64,
    pub latency_ms: i64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub config: GatewayConfig,
    pub providers: Vec<Provider>,
    pub aliases: Vec<ModelAlias>,
    pub summary: UsageSummary,
    pub trend: Vec<UsagePoint>,
    pub provider_stats: Vec<GroupStat>,
    pub model_stats: Vec<GroupStat>,
    pub logs: Vec<RequestLog>,
    pub uptime_seconds: u64,
    pub active_connections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_single_target_alias() {
        let mut alias: ModelAlias = serde_json::from_value(serde_json::json!({
            "id":"legacy","alias":"fast","providerId":"provider-a","upstreamModel":"model-a",
            "client":"all","enabled":true
        }))
        .unwrap();
        alias.normalize();
        assert!(alias.failover_enabled);
        assert_eq!(alias.targets.len(), 1);
        assert_eq!(alias.targets[0].provider_id, "provider-a");
        assert_eq!(alias.targets[0].upstream_model, "model-a");
    }
}
