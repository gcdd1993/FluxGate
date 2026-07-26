use crate::{
    db::Database,
    models::{ClientKind, GatewayConfig, ModelAlias, Provider, RequestLog},
    rectifier,
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    routing::any,
    Router,
};
use chrono::Utc;
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Debug, Clone)]
struct Circuit {
    failures: u32,
    opened_at: Option<Instant>,
}

struct StreamLogContext {
    state: Arc<GatewayState>,
    provider: Provider,
    client: ClientKind,
    model: String,
    started: Instant,
    config: GatewayConfig,
}

struct StreamUsageMeter {
    context: Option<StreamLogContext>,
    pending: String,
    usage: (i64, i64, i64),
    status: i64,
}

impl StreamUsageMeter {
    fn new(context: StreamLogContext, status: StatusCode) -> Self {
        Self {
            context: Some(context),
            pending: String::new(),
            usage: (0, 0, 0),
            status: status.as_u16() as i64,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.pending.push_str(&String::from_utf8_lossy(chunk));
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].trim_end_matches('\r').to_string();
            self.pending.drain(..=newline);
            self.collect_line(&line);
        }
    }

    fn collect_line(&mut self, line: &str) {
        if !line.contains("\"usage\"") {
            return;
        }
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let usage = parse_usage(data.trim().as_bytes());
        self.usage.0 = self.usage.0.max(usage.0);
        self.usage.1 = self.usage.1.max(usage.1);
        self.usage.2 = self.usage.2.max(usage.2);
    }

    async fn finish(&mut self, error: Option<String>) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.collect_line(&pending);
        }
        let Some(context) = self.context.take() else {
            return;
        };
        let status = if error.is_some() { 502 } else { self.status };
        let cost = estimate_cost(&context.model, self.usage.0, self.usage.1, self.usage.2);
        log_request(
            &context.state,
            &context.provider,
            &context.client,
            &context.model,
            self.usage.0,
            self.usage.1,
            self.usage.2,
            cost,
            context.started.elapsed().as_millis() as i64,
            status,
            error,
            &context.config,
        )
        .await;
    }
}

impl Drop for StreamUsageMeter {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.collect_line(&pending);
        }
        let Some(context) = self.context.take() else {
            return;
        };
        let usage = self.usage;
        let status = self.status;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let cost = estimate_cost(&context.model, usage.0, usage.1, usage.2);
                log_request(
                    &context.state,
                    &context.provider,
                    &context.client,
                    &context.model,
                    usage.0,
                    usage.1,
                    usage.2,
                    cost,
                    context.started.elapsed().as_millis() as i64,
                    status,
                    Some("client disconnected before the SSE usage trailer".into()),
                    &context.config,
                )
                .await;
            });
        }
    }
}

pub struct GatewayState {
    pub db: Arc<Database>,
    pub config: RwLock<GatewayConfig>,
    pub providers: RwLock<Vec<Provider>>,
    pub aliases: RwLock<Vec<ModelAlias>>,
    circuits: Mutex<HashMap<String, Circuit>>,
    pub started: Instant,
    pub active: AtomicUsize,
    http: reqwest::Client,
}

impl GatewayState {
    pub fn new(
        db: Arc<Database>,
        config: GatewayConfig,
        providers: Vec<Provider>,
        aliases: Vec<ModelAlias>,
    ) -> Self {
        Self {
            db,
            config: RwLock::new(config),
            providers: RwLock::new(providers),
            aliases: RwLock::new(aliases),
            circuits: Mutex::new(HashMap::new()),
            started: Instant::now(),
            active: AtomicUsize::new(0),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .build()
                .expect("http client"),
        }
    }
    pub async fn reset_circuit(&self, id: &str) {
        self.circuits.lock().await.remove(id);
        let mut providers = self.providers.write().await;
        if let Some(p) = providers.iter_mut().find(|p| p.id == id) {
            p.health = "healthy".into();
            p.failures = 0;
        }
    }

    pub async fn fetch_provider_models(&self, provider: &Provider) -> anyhow::Result<Vec<String>> {
        let url = models_url(&provider.base_url);
        let mut request = self
            .http
            .get(&url)
            .header("accept", "application/json")
            .header("accept-encoding", "identity");
        if let Some(user_agent) = valid_custom_user_agent(provider) {
            request = request.header(http::header::USER_AGENT, user_agent);
        }
        request = if provider.protocol == "anthropic" {
            request
                .header("x-api-key", &provider.api_key)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(&provider.api_key)
        };
        let response = request.send().await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            anyhow::bail!(
                "模型列表请求失败（HTTP {}）：{}",
                status.as_u16(),
                detail.chars().take(300).collect::<String>()
            );
        }
        let value: Value = serde_json::from_slice(&body)?;
        let models = parse_model_ids(&value);
        if models.is_empty() {
            anyhow::bail!("供应商返回成功，但响应中没有可识别的模型 ID");
        }
        Ok(models)
    }
}

fn models_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/models") {
        base.to_string()
    } else if base.ends_with("/v1") || base.ends_with("/api/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

fn parse_model_ids(value: &Value) -> Vec<String> {
    let items = value
        .get("data")
        .or_else(|| value.get("models"))
        .or_else(|| value.pointer("/body/data"))
        .unwrap_or(value);
    let mut models = items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str().map(str::to_owned).or_else(|| {
                item.get("id")
                    .or_else(|| item.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        })
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort_by_key(|id| id.to_lowercase());
    models.dedup();
    models
}

pub async fn serve(state: Arc<GatewayState>, host: String, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", any(proxy))
        .route("/{*path}", any(proxy))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn proxy(State(state): State<Arc<GatewayState>>, req: Request<Body>) -> Response<Body> {
    let started = Instant::now();
    state.active.fetch_add(1, Ordering::SeqCst);
    let result = proxy_inner(state.clone(), req, started).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    result
}

async fn proxy_inner(
    state: Arc<GatewayState>,
    req: Request<Body>,
    started: Instant,
) -> Response<Body> {
    let config = state.config.read().await.clone();
    if !config.enabled {
        return error(StatusCode::SERVICE_UNAVAILABLE, "FluxGate is paused");
    }
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    if path == "/health" {
        return json_response(
            StatusCode::OK,
            json!({"status":"ok","uptime":state.started.elapsed().as_secs()}),
        );
    }
    let client = detect_client(&path);
    if !config.clients.enabled(&client) {
        return error(StatusCode::FORBIDDEN, "Routing for this client is disabled");
    }
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = match to_bytes(req.into_body(), 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, &format!("Invalid body: {e}")),
    };
    let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let original_model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let aliases = state.aliases.read().await.clone();
    let mut alias = aliases
        .iter()
        .find(|a| {
            a.enabled
                && a.alias == original_model
                && (a.client == "all" || a.client == client.as_str())
        })
        .cloned();
    if let Some(alias) = &mut alias {
        alias.normalize();
    }
    let providers = state.providers.read().await.clone();
    let mut attempts: Vec<(Provider, String)> = if let Some(alias) = &alias {
        alias
            .targets
            .iter()
            .filter(|target| target.enabled)
            .filter_map(|target| {
                providers
                    .iter()
                    .find(|provider| {
                        provider.id == target.provider_id
                            && provider.enabled
                            && (provider.client == "all" || provider.client == client.as_str())
                    })
                    .cloned()
                    .map(|provider| (provider, target.upstream_model.clone()))
            })
            .collect()
    } else {
        let mut candidates = providers
            .into_iter()
            .filter(|provider| {
                provider.enabled && (provider.client == "all" || provider.client == client.as_str())
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|provider| provider.priority);
        candidates
            .into_iter()
            .map(|provider| (provider, original_model.clone()))
            .collect()
    };
    if attempts.is_empty() {
        return error(
            StatusCode::BAD_GATEWAY,
            "No enabled provider/model target matches this request",
        );
    }
    let alias_failover = alias
        .as_ref()
        .map(|value| value.failover_enabled)
        .unwrap_or(false);
    let allow_failover = alias_failover || config.auto_failover.enabled(&client);
    let max_attempts = if allow_failover {
        if alias.is_some() {
            attempts.len()
        } else {
            attempts.len().min((config.max_retries + 1) as usize)
        }
    } else {
        1
    };
    attempts.truncate(max_attempts);
    let mut last_error = "No provider attempted".to_string();
    for (provider, routed_model) in attempts {
        if circuit_open(&state, &provider.id, config.recovery_seconds).await {
            continue;
        }
        let mut attempt_payload = payload.clone();
        if let Some(object) = attempt_payload.as_object_mut() {
            object.insert("model".into(), Value::String(routed_model.clone()));
        }
        if provider.protocol == "openai" && path.ends_with("/chat/completions") {
            inject_stream_include_usage(&mut attempt_payload);
        }
        attempt_payload = rectifier::preflight(&attempt_payload, &routed_model, &config.rectifier);
        let mut rectifier_retry_available = true;
        loop {
            match forward(
                &state,
                &provider,
                &client,
                &method,
                &path,
                &query,
                &headers,
                &attempt_payload,
                &body,
            )
            .await
            {
                Ok(upstream) => {
                    let status = upstream.status();
                    let response_headers = upstream.headers().clone();
                    let is_stream = response_headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.contains("text/event-stream"))
                        .unwrap_or(false);
                    if status.is_success() {
                        mark_success(&state, &provider.id).await;
                        if is_stream {
                            let log_context = StreamLogContext {
                                state: state.clone(),
                                provider: provider.clone(),
                                client: client.clone(),
                                model: routed_model.clone(),
                                started,
                                config: config.clone(),
                            };
                            match stream_response(status, response_headers, upstream, log_context)
                                .await
                            {
                                Ok(response) => return response,
                                Err(stream_error) => {
                                    last_error = format!(
                                        "{} returned an SSE error: {}",
                                        provider.name, stream_error
                                    );
                                    mark_failure(&state, &provider.id, config.failure_threshold)
                                        .await;
                                    log_request(
                                        &state,
                                        &provider,
                                        &client,
                                        &routed_model,
                                        0,
                                        0,
                                        0,
                                        0.0,
                                        started.elapsed().as_millis() as i64,
                                        502,
                                        Some(stream_error.chars().take(400).collect()),
                                        &config,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                        let bytes = match upstream.bytes().await {
                            Ok(x) => x,
                            Err(e) => return error(StatusCode::BAD_GATEWAY, &e.to_string()),
                        };
                        let usage = parse_usage(&bytes);
                        let cost = estimate_cost(&routed_model, usage.0, usage.1, usage.2);
                        let bytes = adapt_response(&client, &provider.protocol, bytes);
                        log_request(
                            &state,
                            &provider,
                            &client,
                            &routed_model,
                            usage.0,
                            usage.1,
                            usage.2,
                            cost,
                            started.elapsed().as_millis() as i64,
                            status.as_u16() as i64,
                            None,
                            &config,
                        )
                        .await;
                        return bytes_response(status, response_headers, bytes);
                    }
                    let text = upstream.text().await.unwrap_or_default();
                    if rectifier_retry_available {
                        if let Some(rectified) =
                            rectifier::rectify_for_error(&attempt_payload, &text, &config.rectifier)
                        {
                            attempt_payload = rectified;
                            rectifier_retry_available = false;
                            continue;
                        }
                    }
                    last_error = format!("{} returned {}", provider.name, status);
                    mark_failure(&state, &provider.id, config.failure_threshold).await;
                    log_request(
                        &state,
                        &provider,
                        &client,
                        &routed_model,
                        0,
                        0,
                        0,
                        0.0,
                        started.elapsed().as_millis() as i64,
                        status.as_u16() as i64,
                        Some(text.chars().take(400).collect()),
                        &config,
                    )
                    .await;
                    if !retryable(status) && !alias_failover {
                        return error(status, &last_error);
                    }
                    break;
                }
                Err(e) => {
                    last_error = format!("{}: {e}", provider.name);
                    mark_failure(&state, &provider.id, config.failure_threshold).await;
                    log_request(
                        &state,
                        &provider,
                        &client,
                        &routed_model,
                        0,
                        0,
                        0,
                        0.0,
                        started.elapsed().as_millis() as i64,
                        502,
                        Some(e.to_string()),
                        &config,
                    )
                    .await;
                    break;
                }
            }
        }
    }
    error(
        StatusCode::BAD_GATEWAY,
        &format!("All upstream providers failed. {last_error}"),
    )
}

async fn forward(
    state: &GatewayState,
    p: &Provider,
    client: &ClientKind,
    method: &http::Method,
    path: &str,
    query: &str,
    headers: &HeaderMap,
    payload: &Value,
    raw: &bytes::Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
    let (target_path, send_body) = adapt_request(client, &p.protocol, path, payload, raw);
    let base = p.base_url.trim_end_matches('/');
    let target_path = if base.ends_with("/v1") && target_path.starts_with("/v1/") {
        &target_path[3..]
    } else {
        &target_path
    };
    let url = format!("{base}{target_path}{query}");
    let force_identity_encoding = payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || target_path != path
        || send_body.as_slice() != raw.as_ref();
    let upstream_headers = build_upstream_headers(p, headers, force_identity_encoding);
    let mut builder = state
        .http
        .request(method.clone(), url)
        .headers(upstream_headers);
    builder = if p.protocol == "anthropic" {
        builder.header("x-api-key", &p.api_key).header(
            "anthropic-version",
            headers
                .get("anthropic-version")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("2023-06-01"),
        )
    } else {
        builder.bearer_auth(&p.api_key)
    };
    builder.body(send_body).send().await
}

fn valid_custom_user_agent(provider: &Provider) -> Option<HeaderValue> {
    provider
        .custom_user_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| HeaderValue::from_str(value).ok())
}

fn build_upstream_headers(
    provider: &Provider,
    incoming: &HeaderMap,
    force_identity_encoding: bool,
) -> HeaderMap {
    let mut outgoing = HeaderMap::new();
    let custom_user_agent = valid_custom_user_agent(provider);

    for (name, value) in incoming {
        let key = name.as_str();
        if is_stripped_upstream_header(key) {
            continue;
        }
        if key.eq_ignore_ascii_case("accept-encoding") && force_identity_encoding {
            continue;
        }
        if key.eq_ignore_ascii_case("user-agent") {
            if custom_user_agent.is_some() {
                continue;
            }
        }
        outgoing.append(name.clone(), value.clone());
    }

    if force_identity_encoding {
        outgoing.insert(
            http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
    }
    if let Some(user_agent) = custom_user_agent {
        outgoing.insert(http::header::USER_AGENT, user_agent);
    }
    outgoing
}

fn is_stripped_upstream_header(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "content-length"
            | "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "forwarded"
            | "x-forwarded-host"
            | "x-forwarded-port"
            | "x-forwarded-proto"
            | "cf-connecting-ip"
            | "cf-ipcountry"
            | "cf-ray"
            | "cf-visitor"
            | "true-client-ip"
            | "x-request-id"
            | "x-correlation-id"
            | "x-trace-id"
            | "x-amzn-trace-id"
            | "traceparent"
            | "tracestate"
    )
}

fn adapt_request(
    client: &ClientKind,
    protocol: &str,
    path: &str,
    payload: &Value,
    raw: &bytes::Bytes,
) -> (String, Vec<u8>) {
    if matches!(client, ClientKind::Codex) && protocol == "openai" && path.ends_with("/responses") {
        let input = payload
            .get("input")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let messages = match input {
            Value::Array(v) => Value::Array(v),
            Value::String(s) => json!([{"role":"user","content":s}]),
            _ => json!([]),
        };
        let body = json!({"model":payload.get("model"),"messages":messages,"stream":payload.get("stream").and_then(Value::as_bool).unwrap_or(false)});
        return (
            "/v1/chat/completions".into(),
            serde_json::to_vec(&body).unwrap_or_default(),
        );
    }
    if matches!(client, ClientKind::Claude) && protocol == "openai" && path.ends_with("/messages") {
        let mut messages = payload.get("messages").cloned().unwrap_or(json!([]));
        if let (Some(system), Some(items)) = (payload.get("system"), messages.as_array_mut()) {
            items.insert(0, json!({"role":"system","content":system}));
        }
        let body = json!({"model":payload.get("model"),"messages":messages,"stream":payload.get("stream").and_then(Value::as_bool).unwrap_or(false),"max_tokens":payload.get("max_tokens")});
        return (
            "/v1/chat/completions".into(),
            serde_json::to_vec(&body).unwrap_or_default(),
        );
    }
    (
        path.to_string(),
        if payload.is_null() {
            raw.to_vec()
        } else {
            serde_json::to_vec(payload).unwrap_or_else(|_| raw.to_vec())
        },
    )
}

fn inject_stream_include_usage(payload: &mut Value) {
    if payload.get("stream").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    match object.get_mut("stream_options") {
        Some(Value::Object(options)) => {
            options.insert("include_usage".into(), Value::Bool(true));
        }
        _ => {
            object.insert("stream_options".into(), json!({"include_usage":true}));
        }
    }
}

fn adapt_response(client: &ClientKind, protocol: &str, raw: bytes::Bytes) -> bytes::Bytes {
    if protocol != "openai" {
        return raw;
    }
    let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
        return raw;
    };
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let model = v.get("model").and_then(Value::as_str).unwrap_or("unknown");
    let input = v
        .pointer("/usage/prompt_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = v
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let converted = match client {
        ClientKind::Claude => {
            json!({"id":v.get("id"),"type":"message","role":"assistant","model":model,"content":[{"type":"text","text":text}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":input,"output_tokens":output}})
        }
        ClientKind::Codex => {
            json!({"id":v.get("id"),"object":"response","created_at":Utc::now().timestamp(),"status":"completed","model":model,"output":[{"type":"message","id":format!("msg_{}",uuid::Uuid::new_v4()),"status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]}],"usage":{"input_tokens":input,"output_tokens":output,"total_tokens":input+output}})
        }
        ClientKind::Openai => return raw,
    };
    bytes::Bytes::from(serde_json::to_vec(&converted).unwrap_or_else(|_| raw.to_vec()))
}

fn detect_client(path: &str) -> ClientKind {
    if path.ends_with("/messages") {
        ClientKind::Claude
    } else if path.ends_with("/responses") {
        ClientKind::Codex
    } else {
        ClientKind::Openai
    }
}
async fn circuit_open(state: &GatewayState, id: &str, recovery: u64) -> bool {
    let mut map = state.circuits.lock().await;
    if let Some(c) = map.get_mut(id) {
        if let Some(t) = c.opened_at {
            if t.elapsed() < Duration::from_secs(recovery) {
                return true;
            }
            c.opened_at = None;
            c.failures = 0;
        }
    }
    false
}
async fn mark_failure(state: &GatewayState, id: &str, threshold: u32) {
    let mut map = state.circuits.lock().await;
    let c = map.entry(id.into()).or_insert(Circuit {
        failures: 0,
        opened_at: None,
    });
    c.failures += 1;
    if c.failures >= threshold {
        c.opened_at = Some(Instant::now());
    }
    let mut p = state.providers.write().await;
    if let Some(x) = p.iter_mut().find(|x| x.id == id) {
        x.failures = c.failures as i64;
        x.health = if c.opened_at.is_some() {
            "circuit_open".into()
        } else {
            "warning".into()
        };
    }
}
async fn mark_success(state: &GatewayState, id: &str) {
    state.circuits.lock().await.remove(id);
    let mut p = state.providers.write().await;
    if let Some(x) = p.iter_mut().find(|x| x.id == id) {
        x.failures = 0;
        x.health = "healthy".into();
    }
}
fn retryable(s: StatusCode) -> bool {
    s == StatusCode::REQUEST_TIMEOUT || s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}
fn parse_usage(bytes: &[u8]) -> (i64, i64, i64) {
    let v: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let u = v.get("usage").unwrap_or(&Value::Null);
    let input = u
        .get("input_tokens")
        .or_else(|| u.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = u
        .get("output_tokens")
        .or_else(|| u.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = u
        .pointer("/input_tokens_details/cached_tokens")
        .or_else(|| u.pointer("/prompt_tokens_details/cached_tokens"))
        .or_else(|| u.get("cache_read_input_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    (input, output, cached)
}
#[cfg(test)]
fn parse_stream_usage(bytes: &[u8]) -> (i64, i64, i64) {
    let text = String::from_utf8_lossy(bytes);
    let mut result = (0, 0, 0);
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let usage = parse_usage(data.as_bytes());
        result.0 = result.0.max(usage.0);
        result.1 = result.1.max(usage.1);
        result.2 = result.2.max(usage.2);
    }
    result
}
fn estimate_cost(model: &str, input: i64, output: i64, cached: i64) -> f64 {
    let (in_rate, out_rate, cache_rate) = if model.contains("claude") {
        (3.0, 15.0, 0.3)
    } else if model.contains("gpt") {
        (1.25, 10.0, 0.125)
    } else {
        (0.5, 2.0, 0.05)
    };
    ((input as f64 * in_rate) + (output as f64 * out_rate) + (cached as f64 * cache_rate))
        / 1_000_000.0
}
async fn log_request(
    state: &GatewayState,
    p: &Provider,
    c: &ClientKind,
    model: &str,
    input: i64,
    output: i64,
    cached: i64,
    cost: f64,
    latency: i64,
    status: i64,
    error: Option<String>,
    config: &GatewayConfig,
) {
    if !config.record_usage {
        return;
    }
    let l = RequestLog {
        id: 0,
        timestamp: Utc::now().to_rfc3339(),
        client: c.as_str().into(),
        provider_name: p.name.clone(),
        provider_id: p.id.clone(),
        model: model.into(),
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached,
        cost,
        latency_ms: latency,
        status,
        error,
    };
    let _ = state.db.insert_log(&l);
}
fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(
        status,
        json!({"error":{"message":message,"type":"fluxgate_error"}}),
    )
}
fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}
fn bytes_response(status: StatusCode, headers: HeaderMap, body: bytes::Bytes) -> Response<Body> {
    let mut out = Response::builder().status(status);
    if let Some(ct) = headers.get("content-type") {
        out = out.header("content-type", ct);
    }
    out.body(Body::from(body)).unwrap()
}
async fn stream_response(
    status: StatusCode,
    headers: HeaderMap,
    response: reqwest::Response,
    log_context: StreamLogContext,
) -> Result<Response<Body>, String> {
    let mut upstream = response.bytes_stream();
    let first = upstream
        .next()
        .await
        .ok_or_else(|| "upstream SSE stream ended before the first event".to_string())?
        .map_err(|error| format!("failed to read upstream SSE: {error}"))?;
    let preview = String::from_utf8_lossy(&first);
    if is_terminal_sse_error(&preview) {
        return Err(preview.chars().take(400).collect());
    }
    let mut out = Response::builder().status(status);
    for name in ["content-type", "cache-control", "x-request-id"] {
        if let (Some(n), Some(v)) = (
            HeaderName::from_bytes(name.as_bytes()).ok(),
            headers.get(name),
        ) {
            if let Ok(v) = HeaderValue::from_bytes(v.as_bytes()) {
                out = out.header(n, v);
            }
        }
    }
    let combined: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>> =
        Box::pin(stream::once(async move { Ok(first) }).chain(upstream));
    let meter = StreamUsageMeter::new(log_context, status);
    let metered = stream::unfold(
        (combined, meter),
        move |(mut upstream, mut meter)| async move {
            match upstream.next().await {
                Some(Ok(chunk)) => {
                    meter.push(&chunk);
                    Some((Ok::<_, reqwest::Error>(chunk), (upstream, meter)))
                }
                Some(Err(error)) => {
                    meter.finish(Some(error.to_string())).await;
                    Some((Err(error), (upstream, meter)))
                }
                None => {
                    meter.finish(None).await;
                    None
                }
            }
        },
    );
    Ok(out.body(Body::from_stream(metered)).unwrap())
}

fn is_terminal_sse_error(chunk: &str) -> bool {
    let lower = chunk.to_lowercase();
    let error_shape = lower.contains("\"error\"")
        || lower.contains("event: error")
        || lower.contains("resourceexhausted")
        || lower.contains("resource_exhausted");
    let terminal_signal = lower.contains("request limit")
        || lower.contains("rate limit")
        || lower.contains("resource exhausted")
        || lower.contains("resourceexhausted")
        || lower.contains("resource_exhausted")
        || lower.contains("overloaded")
        || lower.contains("insufficient quota")
        || lower.contains("upstream error");
    error_shape && terminal_signal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_codex_responses_request_to_chat_completions() {
        let payload = json!({"model":"gpt-test","input":"hello","stream":false});
        let (path, body) = adapt_request(
            &ClientKind::Codex,
            "openai",
            "/v1/responses",
            &payload,
            &bytes::Bytes::new(),
        );
        let converted: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(path, "/v1/chat/completions");
        assert_eq!(
            converted
                .pointer("/messages/0/content")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn converts_openai_response_back_to_anthropic_message() {
        let raw=bytes::Bytes::from_static(br#"{"id":"chat_1","model":"gpt-test","choices":[{"message":{"content":"done"}}],"usage":{"prompt_tokens":12,"completion_tokens":4}}"#);
        let converted = adapt_response(&ClientKind::Claude, "openai", raw);
        let value: Value = serde_json::from_slice(&converted).unwrap();
        assert_eq!(value.get("type").and_then(Value::as_str), Some("message"));
        assert_eq!(
            value.pointer("/content/0/text").and_then(Value::as_str),
            Some("done")
        );
        assert_eq!(
            value.pointer("/usage/input_tokens").and_then(Value::as_i64),
            Some(12)
        );
    }

    #[test]
    fn detects_resource_exhausted_inside_successful_sse_status() {
        assert!(is_terminal_sse_error(
            "data: {\"error\":{\"message\":\"ResourceExhausted: Worker local total request limit reached (1928/48)\"}}\n\n"
        ));
        assert!(!is_terminal_sse_error(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n"
        ));
    }

    #[test]
    fn parses_usage_from_final_openai_sse_chunk() {
        let stream = br#"data: {"choices":[{"delta":{"content":"OK"}}],"usage":null}

data: {"choices":[],"usage":{"prompt_tokens":1234,"completion_tokens":56,"total_tokens":1290,"prompt_tokens_details":{"cached_tokens":321}}}

data: [DONE]

"#;
        assert_eq!(parse_stream_usage(stream), (1234, 56, 321));
    }

    #[test]
    fn injects_openai_stream_usage_without_overwriting_existing_options() {
        let mut payload = json!({
            "stream": true,
            "stream_options": {"continuous_usage_stats": true}
        });
        inject_stream_include_usage(&mut payload);
        assert_eq!(payload["stream_options"]["include_usage"], true);
        assert_eq!(payload["stream_options"]["continuous_usage_stats"], true);

        let mut non_stream = json!({"stream":false});
        inject_stream_include_usage(&mut non_stream);
        assert!(non_stream.get("stream_options").is_none());
    }

    #[test]
    fn builds_models_endpoint_for_common_provider_urls() {
        assert_eq!(
            models_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_url("https://example.test/v1/models"),
            "https://example.test/v1/models"
        );
    }

    #[test]
    fn parses_openai_anthropic_and_simple_model_lists() {
        assert_eq!(
            parse_model_ids(&json!({"data":[{"id":"gpt-z"},{"id":"gpt-a"}]})),
            vec!["gpt-a", "gpt-z"]
        );
        assert_eq!(
            parse_model_ids(&json!({"models":[{"name":"claude-test"}]})),
            vec!["claude-test"]
        );
        assert_eq!(
            parse_model_ids(&json!(["model-b", "model-a"])),
            vec!["model-a", "model-b"]
        );
    }

    #[test]
    fn upstream_headers_apply_custom_user_agent_and_sse_hygiene() {
        let provider = Provider {
            id: "test".into(),
            name: "Test".into(),
            protocol: "openai".into(),
            base_url: "https://example.com/v1".into(),
            api_key: "secret".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: Some("provider-client/1.0".into()),
        };
        let mut incoming = HeaderMap::new();
        incoming.insert("user-agent", HeaderValue::from_static("workbuddy/1.0"));
        incoming.insert("accept-encoding", HeaderValue::from_static("gzip, br"));
        incoming.insert("authorization", HeaderValue::from_static("Bearer inbound"));
        incoming.insert("connection", HeaderValue::from_static("keep-alive"));
        incoming.insert("cf-ray", HeaderValue::from_static("trace"));
        incoming.insert("content-type", HeaderValue::from_static("application/json"));

        let outgoing = build_upstream_headers(&provider, &incoming, true);
        assert_eq!(outgoing.get("user-agent").unwrap(), "provider-client/1.0");
        assert_eq!(outgoing.get("accept-encoding").unwrap(), "identity");
        assert_eq!(outgoing.get("content-type").unwrap(), "application/json");
        assert!(outgoing.get("authorization").is_none());
        assert!(outgoing.get("connection").is_none());
        assert!(outgoing.get("cf-ray").is_none());
    }

    #[test]
    fn upstream_headers_preserve_client_fingerprint_without_override() {
        let provider = Provider {
            id: "test".into(),
            name: "Test".into(),
            protocol: "openai".into(),
            base_url: "https://example.com/v1".into(),
            api_key: "secret".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
        };
        let mut incoming = HeaderMap::new();
        incoming.insert("user-agent", HeaderValue::from_static("workbuddy/1.0"));
        incoming.insert("accept-encoding", HeaderValue::from_static("gzip"));

        let outgoing = build_upstream_headers(&provider, &incoming, false);
        assert_eq!(outgoing.get("user-agent").unwrap(), "workbuddy/1.0");
        assert_eq!(outgoing.get("accept-encoding").unwrap(), "gzip");
    }

    #[tokio::test]
    async fn fetches_models_with_provider_authentication() {
        let app = Router::new().route(
            "/v1/models",
            axum::routing::get(|headers: HeaderMap| async move {
                assert_eq!(headers.get("authorization").unwrap(), "Bearer test-key");
                axum::Json(json!({"data":[{"id":"model-z"},{"id":"model-a"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let path =
            std::env::temp_dir().join(format!("fluxgate-models-test-{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::open(&path).unwrap());
        let state = GatewayState::new(db, GatewayConfig::default(), Vec::new(), Vec::new());
        let provider = Provider {
            id: "test".into(),
            name: "Test".into(),
            protocol: "openai".into(),
            base_url: format!("http://{address}"),
            api_key: "test-key".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
        };
        assert_eq!(
            state.fetch_provider_models(&provider).await.unwrap(),
            vec!["model-a", "model-z"]
        );
        server.abort();
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn alias_fails_over_to_next_provider_and_model() {
        let failed_app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let failed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let failed_address = failed_listener.local_addr().unwrap();
        let failed_server =
            tokio::spawn(async move { axum::serve(failed_listener, failed_app).await.unwrap() });

        let healthy_app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|axum::Json(body): axum::Json<Value>| async move {
                axum::Json(json!({"id":"ok","model":body["model"],"choices":[{"message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
            }),
        );
        let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let healthy_address = healthy_listener.local_addr().unwrap();
        let healthy_server =
            tokio::spawn(async move { axum::serve(healthy_listener, healthy_app).await.unwrap() });

        let provider = |id: &str, address| Provider {
            id: id.into(),
            name: id.into(),
            protocol: "openai".into(),
            base_url: format!("http://{address}/v1"),
            api_key: "test".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
        };
        let providers = vec![
            provider("p1", failed_address),
            provider("p2", healthy_address),
        ];
        let alias = ModelAlias {
            id: "alias-1".into(),
            alias: "fast".into(),
            targets: vec![
                crate::models::AliasTarget {
                    id: "t1".into(),
                    provider_id: "p1".into(),
                    upstream_model: "model-primary".into(),
                    enabled: true,
                },
                crate::models::AliasTarget {
                    id: "t2".into(),
                    provider_id: "p2".into(),
                    upstream_model: "model-backup".into(),
                    enabled: true,
                },
            ],
            provider_id: String::new(),
            upstream_model: String::new(),
            client: "all".into(),
            enabled: true,
            failover_enabled: true,
        };
        let path = std::env::temp_dir().join(format!(
            "fluxgate-failover-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::open(&path).unwrap());
        let state = Arc::new(GatewayState::new(
            db,
            GatewayConfig::default(),
            providers,
            vec![alias],
        ));
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"model":"fast","messages":[]}).to_string(),
            ))
            .unwrap();
        let response = proxy_inner(state.clone(), request, Instant::now()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["model"], "model-backup");
        failed_server.abort();
        healthy_server.abort();
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn rectifier_retries_same_provider_without_workbuddy_thinking() {
        let calls = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let handler_calls = calls.clone();
        let handler_bodies = bodies.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let calls = handler_calls.clone();
                let bodies = handler_bodies.clone();
                async move {
                    bodies.lock().unwrap().push(body.clone());
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error":{"message":"Validation: Unsupported parameter(s): `thinking`"}})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            axum::Json(json!({"id":"ok","model":body["model"],"choices":[{"message":{"content":"done"}}]})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = Provider {
            id: "p1".into(),
            name: "p1".into(),
            protocol: "openai".into(),
            base_url: format!("http://{address}/v1"),
            api_key: "test".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
        };
        let alias = ModelAlias {
            id: "alias-1".into(),
            alias: "deepseek-v4-flash".into(),
            targets: vec![crate::models::AliasTarget {
                id: "t1".into(),
                provider_id: "p1".into(),
                upstream_model: "deepseek-ai/deepseek-v4-flash".into(),
                enabled: true,
            }],
            provider_id: String::new(),
            upstream_model: String::new(),
            client: "all".into(),
            enabled: true,
            failover_enabled: true,
        };
        let path = std::env::temp_dir().join(format!(
            "fluxgate-rectifier-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::open(&path).unwrap());
        let state = Arc::new(GatewayState::new(
            db,
            GatewayConfig::default(),
            vec![provider],
            vec![alias],
        ));
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"model":"deepseek-v4-flash","thinking":{"type":"enabled"},"messages":[]})
                    .to_string(),
            ))
            .unwrap();
        let response = proxy_inner(state.clone(), request, Instant::now()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let bodies = bodies.lock().unwrap();
        assert!(bodies[0].get("thinking").is_some());
        assert!(bodies[1].get("thinking").is_none());
        assert_eq!(bodies[1]["model"], "deepseek-ai/deepseek-v4-flash");
        server.abort();
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn alias_fails_over_when_first_sse_chunk_contains_resource_exhausted() {
        let failed_app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from("data: {\"error\":{\"message\":\"ResourceExhausted: Worker local total request limit reached (1928/48)\"}}\n\n"))
                    .unwrap()
            }),
        );
        let failed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let failed_address = failed_listener.local_addr().unwrap();
        let failed_server =
            tokio::spawn(async move { axum::serve(failed_listener, failed_app).await.unwrap() });

        let healthy_app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"backup-ok\"}}]}\n\ndata: [DONE]\n\n"))
                    .unwrap()
            }),
        );
        let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let healthy_address = healthy_listener.local_addr().unwrap();
        let healthy_server =
            tokio::spawn(async move { axum::serve(healthy_listener, healthy_app).await.unwrap() });

        let provider = |id: &str, address| Provider {
            id: id.into(),
            name: id.into(),
            protocol: "openai".into(),
            base_url: format!("http://{address}/v1"),
            api_key: "test".into(),
            enabled: true,
            priority: 1,
            client: "all".into(),
            health: "healthy".into(),
            failures: 0,
            models: Vec::new(),
            custom_user_agent: None,
        };
        let alias = ModelAlias {
            id: "alias-sse".into(),
            alias: "deepseek-v4-flash".into(),
            targets: vec![
                crate::models::AliasTarget {
                    id: "t1".into(),
                    provider_id: "p1".into(),
                    upstream_model: "primary-model".into(),
                    enabled: true,
                },
                crate::models::AliasTarget {
                    id: "t2".into(),
                    provider_id: "p2".into(),
                    upstream_model: "backup-model".into(),
                    enabled: true,
                },
            ],
            provider_id: String::new(),
            upstream_model: String::new(),
            client: "all".into(),
            enabled: true,
            failover_enabled: true,
        };
        let path = std::env::temp_dir().join(format!(
            "fluxgate-sse-failover-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::open(&path).unwrap());
        let state = Arc::new(GatewayState::new(
            db,
            GatewayConfig::default(),
            vec![
                provider("p1", failed_address),
                provider("p2", healthy_address),
            ],
            vec![alias],
        ));
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"model":"deepseek-v4-flash","messages":[],"stream":true}).to_string(),
            ))
            .unwrap();
        let response = proxy_inner(state.clone(), request, Instant::now()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("backup-ok"));
        failed_server.abort();
        healthy_server.abort();
        drop(state);
        let _ = std::fs::remove_file(path);
    }
}
