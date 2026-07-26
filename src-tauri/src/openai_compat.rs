//! Full protocol translation between the Anthropic Messages API and
//! OpenAI-compatible chat/completions upstreams, so Claude-family clients
//! (Claude Code included) can ride providers that only expose the OpenAI
//! surface: tools, images and streaming all round-trip.

use serde_json::{json, Map, Value};

/// Convert an Anthropic `/v1/messages` payload into an OpenAI
/// `/v1/chat/completions` payload.
pub fn claude_request_to_openai(payload: &Value) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = payload.get("system") {
        let text = collect_text(system);
        if !text.is_empty() {
            messages.push(json!({"role": "system", "content": text}));
        }
    }
    let empty = Vec::new();
    for message in payload
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        convert_message(message, &mut messages);
    }
    let stream = payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut body = Map::new();
    body.insert(
        "model".into(),
        payload.get("model").cloned().unwrap_or(Value::Null),
    );
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), Value::Bool(stream));
    if stream {
        // The usage trailer is what feeds FluxGate's request log.
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    for key in ["max_tokens", "temperature", "top_p"] {
        if let Some(value) = payload.get(key) {
            body.insert(key.into(), value.clone());
        }
    }
    if let Some(stop) = payload.get("stop_sequences") {
        body.insert("stop".into(), stop.clone());
    }
    if let Some(user) = payload.pointer("/metadata/user_id").and_then(Value::as_str) {
        body.insert("user".into(), json!(user));
    }
    if let Some(tools) = convert_tools(payload.get("tools")) {
        body.insert("tools".into(), tools);
    }
    if let Some(choice) = payload.get("tool_choice") {
        if let Some(mapped) = convert_tool_choice(choice) {
            body.insert("tool_choice".into(), mapped);
        }
        if choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            body.insert("parallel_tool_calls".into(), Value::Bool(false));
        }
    }
    Value::Object(body)
}

fn convert_message(message: &Value, out: &mut Vec<Value>) {
    let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
    let content = message.get("content").cloned().unwrap_or(Value::Null);
    let Some(blocks) = content.as_array() else {
        out.push(json!({"role": role, "content": content}));
        return;
    };
    if role == "assistant" {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                }
                Some("tool_use") => tool_calls.push(json!({
                    "id": block.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": block.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": block
                            .get("input")
                            .map(Value::to_string)
                            .unwrap_or_else(|| "{}".into()),
                    }
                })),
                // thinking / redacted_thinking have no OpenAI equivalent.
                _ => {}
            }
        }
        let mut item = Map::new();
        item.insert("role".into(), json!("assistant"));
        item.insert(
            "content".into(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            },
        );
        if !tool_calls.is_empty() {
            item.insert("tool_calls".into(), Value::Array(tool_calls));
        }
        out.push(Value::Object(item));
        return;
    }
    // User turns: tool results become their own `tool` messages (OpenAI
    // requires them right after the assistant tool_calls turn), everything
    // else regroups into a single user message.
    let mut parts = Vec::new();
    let mut has_image = false;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_result") => out.push(json!({
                "role": "tool",
                "tool_call_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "content": collect_text(block.get("content").unwrap_or(&Value::Null)),
            })),
            Some("text") => parts.push(json!({
                "type": "text",
                "text": block.get("text").cloned().unwrap_or(json!("")),
            })),
            Some("image") => {
                if let Some(url) = image_url(block) {
                    has_image = true;
                    parts.push(json!({"type": "image_url", "image_url": {"url": url}}));
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return;
    }
    let content = if has_image {
        Value::Array(parts)
    } else {
        // Text-only content collapses to a plain string for maximum
        // provider compatibility.
        Value::String(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    };
    out.push(json!({"role": "user", "content": content}));
}

fn image_url(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => Some(format!(
            "data:{};base64,{}",
            source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png"),
            source.get("data").and_then(Value::as_str).unwrap_or("")
        )),
        Some("url") => source
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Extract plain text from a string or an Anthropic content-block array.
fn collect_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

fn convert_tools(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    let mapped: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            // Server tools (web search etc.) carry no schema to forward.
            let name = tool.get("name").and_then(Value::as_str)?;
            let schema = tool.get("input_schema")?;
            let mut function = Map::new();
            function.insert("name".into(), json!(name));
            if let Some(description) = tool.get("description") {
                function.insert("description".into(), description.clone());
            }
            function.insert("parameters".into(), schema.clone());
            Some(json!({"type": "function", "function": function}))
        })
        .collect();
    (!mapped.is_empty()).then_some(Value::Array(mapped))
}

fn convert_tool_choice(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str) {
        Some("auto") => Some(json!("auto")),
        Some("any") => Some(json!("required")),
        Some("none") => Some(json!("none")),
        Some("tool") => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({"type": "function", "function": {"name": name}})),
        _ => None,
    }
}

/// Convert a non-streaming OpenAI chat/completions response into an
/// Anthropic message.
pub fn openai_response_to_claude(response: &Value) -> Value {
    let message = response
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            content.push(json!({"type": "text", "text": text}));
        }
    }
    let empty = Vec::new();
    for (position, call) in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .enumerate()
    {
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("");
        let input: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("toolu_{position}"));
        content.push(json!({
            "type": "tool_use",
            "id": id,
            "name": call.pointer("/function/name").and_then(Value::as_str).unwrap_or(""),
            "input": input,
        }));
    }
    if content.is_empty() {
        content.push(json!({"type": "text", "text": ""}));
    }
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    json!({
        "id": response.get("id").cloned().unwrap_or(json!("msg_fluxgate")),
        "type": "message",
        "role": "assistant",
        "model": response.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "content": content,
        "stop_reason": map_finish_reason(
            response
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
        ),
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": usage.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").and_then(Value::as_i64).unwrap_or(0),
            "cache_read_input_tokens": usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        },
    })
}

fn map_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("length") => "max_tokens",
        _ => "end_turn",
    }
}

enum OpenBlock {
    Text {
        index: usize,
    },
    ToolUse {
        index: usize,
        call_index: i64,
        id: String,
    },
}

/// Stateful translator that rewrites an OpenAI `chat.completion.chunk` SSE
/// stream into the Anthropic Messages event sequence (`message_start`,
/// `content_block_*`, `message_delta`, `message_stop`). Feed it raw upstream
/// bytes; it buffers partial lines across chunk boundaries.
pub struct SseTranslator {
    pending: String,
    started: bool,
    closed: bool,
    open_block: Option<OpenBlock>,
    next_index: usize,
    stop_reason: Option<&'static str>,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
}

impl SseTranslator {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            started: false,
            closed: false,
            open_block: None,
            next_index: 0,
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = String::new();
        self.pending.push_str(&String::from_utf8_lossy(chunk));
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].trim_end_matches('\r').to_string();
            self.pending.drain(..=newline);
            self.handle_line(line.trim(), &mut out);
        }
        out.into_bytes()
    }

    /// Flush at end-of-stream: upstreams that never send `[DONE]` still get a
    /// terminated Anthropic message.
    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = String::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.handle_line(line.trim(), &mut out);
        }
        self.close(&mut out);
        out.into_bytes()
    }

    fn handle_line(&mut self, line: &str, out: &mut String) {
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() {
            return;
        }
        if data == "[DONE]" {
            self.close(out);
            return;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return;
        };
        self.handle_chunk(&chunk, out);
    }

    fn handle_chunk(&mut self, chunk: &Value, out: &mut String) {
        if self.closed {
            return;
        }
        if !self.started {
            self.started = true;
            emit(
                out,
                "message_start",
                &json!({
                    "type": "message_start",
                    "message": {
                        "id": chunk.get("id").and_then(Value::as_str).unwrap_or("msg_fluxgate"),
                        "type": "message",
                        "role": "assistant",
                        "model": chunk.get("model").and_then(Value::as_str).unwrap_or("unknown"),
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {"input_tokens": 0, "output_tokens": 0},
                    }
                }),
            );
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.input_tokens = self
                .input_tokens
                .max(usage.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0));
            self.output_tokens = self.output_tokens.max(
                usage
                    .get("completion_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            );
            self.cached_tokens = self.cached_tokens.max(
                usage
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            );
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    let index = self.ensure_text_block(out);
                    emit(
                        out,
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "text_delta", "text": text},
                        }),
                    );
                }
            }
            let empty = Vec::new();
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .unwrap_or(&empty)
            {
                self.handle_tool_delta(call, out);
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(Some(reason)));
        }
    }

    fn ensure_text_block(&mut self, out: &mut String) -> usize {
        if let Some(OpenBlock::Text { index }) = &self.open_block {
            return *index;
        }
        self.close_block(out);
        let index = self.next_index;
        self.next_index += 1;
        emit(
            out,
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "text", "text": ""},
            }),
        );
        self.open_block = Some(OpenBlock::Text { index });
        index
    }

    fn handle_tool_delta(&mut self, call: &Value, out: &mut String) {
        let call_index = call.get("index").and_then(Value::as_i64).unwrap_or(0);
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or("");
        // A fragment opens a new tool_use block when the OpenAI call index
        // moves on, or when a fresh id shows up on an index-less stream.
        let is_new = match &self.open_block {
            Some(OpenBlock::ToolUse {
                call_index: open_index,
                id: open_id,
                ..
            }) => *open_index != call_index || (!call_id.is_empty() && call_id != open_id),
            _ => true,
        };
        if is_new {
            self.close_block(out);
            let index = self.next_index;
            self.next_index += 1;
            let id = if call_id.is_empty() {
                format!("toolu_{index}")
            } else {
                call_id.to_string()
            };
            emit(
                out,
                "content_block_start",
                &json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": id,
                        "name": call.pointer("/function/name").and_then(Value::as_str).unwrap_or(""),
                        "input": {},
                    },
                }),
            );
            self.open_block = Some(OpenBlock::ToolUse {
                index,
                call_index,
                id,
            });
        }
        if let Some(OpenBlock::ToolUse { index, .. }) = &self.open_block {
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                if !arguments.is_empty() {
                    emit(
                        out,
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": *index,
                            "delta": {"type": "input_json_delta", "partial_json": arguments},
                        }),
                    );
                }
            }
        }
    }

    fn close_block(&mut self, out: &mut String) {
        if let Some(block) = self.open_block.take() {
            let index = match block {
                OpenBlock::Text { index } | OpenBlock::ToolUse { index, .. } => index,
            };
            emit(
                out,
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": index}),
            );
        }
    }

    fn close(&mut self, out: &mut String) {
        if self.closed {
            return;
        }
        self.closed = true;
        if !self.started {
            // Stream ended before any chunk: still hand the client a
            // complete, empty Anthropic message envelope.
            self.started = true;
            emit(
                out,
                "message_start",
                &json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_fluxgate",
                        "type": "message",
                        "role": "assistant",
                        "model": "unknown",
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {"input_tokens": 0, "output_tokens": 0},
                    }
                }),
            );
        }
        self.close_block(out);
        emit(
            out,
            "message_delta",
            &json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": self.stop_reason.unwrap_or("end_turn"),
                    "stop_sequence": Value::Null,
                },
                "usage": {
                    "input_tokens": self.input_tokens,
                    "output_tokens": self.output_tokens,
                    "cache_read_input_tokens": self.cached_tokens,
                },
            }),
        );
        emit(out, "message_stop", &json!({"type": "message_stop"}));
    }
}

fn emit(out: &mut String, event: &str, data: &Value) {
    out.push_str("event: ");
    out.push_str(event);
    out.push_str("\ndata: ");
    out.push_str(&data.to_string());
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_full_claude_request() {
        let payload = json!({
            "model": "claude-sonnet",
            "max_tokens": 1024,
            "temperature": 0.5,
            "top_p": 0.9,
            "stream": true,
            "stop_sequences": ["END"],
            "metadata": {"user_id": "u-1"},
            "system": [{"type": "text", "text": "be brief"}],
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "name": "get_weather",
                "description": "Weather lookup",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}
            }],
            "tool_choice": {"type": "any", "disable_parallel_tool_use": true},
        });
        let body = claude_request_to_openai(&payload);
        assert_eq!(body["model"], "claude-sonnet");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["stop"][0], "END");
        assert_eq!(body["user"], "u-1");
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn converts_tool_use_and_results() {
        let payload = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "checking"},
                    {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "sh"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "sunny"}]},
                    {"type": "text", "text": "and tomorrow?"}
                ]}
            ]
        });
        let body = claude_request_to_openai(&payload);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "checking");
        assert_eq!(messages[1]["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        let arguments: Value = serde_json::from_str(
            messages[1]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(arguments["city"], "sh");
        // Tool result must precede the follow-up user text.
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "toolu_1");
        assert_eq!(messages[2]["content"], "sunny");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[3]["content"], "and tomorrow?");
    }

    #[test]
    fn converts_image_blocks_to_data_uri() {
        let payload = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what is this"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "abc123"}}
            ]}]
        });
        let body = claude_request_to_openai(&payload);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,abc123"
        );
    }

    #[test]
    fn converts_tool_call_response() {
        let response = json!({
            "id": "chat_1",
            "model": "m",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"sh\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3,
                      "prompt_tokens_details": {"cached_tokens": 2}}
        });
        let message = openai_response_to_claude(&response);
        assert_eq!(message["stop_reason"], "tool_use");
        assert_eq!(message["content"][0]["type"], "tool_use");
        assert_eq!(message["content"][0]["id"], "call_9");
        assert_eq!(message["content"][0]["input"]["city"], "sh");
        assert_eq!(message["usage"]["input_tokens"], 7);
        assert_eq!(message["usage"]["output_tokens"], 3);
        assert_eq!(message["usage"]["cache_read_input_tokens"], 2);
    }

    fn events(bytes: &[u8]) -> Vec<(String, Value)> {
        String::from_utf8_lossy(bytes)
            .split("\n\n")
            .filter(|frame| !frame.trim().is_empty())
            .map(|frame| {
                let mut event = String::new();
                let mut data = Value::Null;
                for line in frame.lines() {
                    if let Some(name) = line.strip_prefix("event: ") {
                        event = name.to_string();
                    } else if let Some(payload) = line.strip_prefix("data: ") {
                        data = serde_json::from_str(payload).unwrap();
                    }
                }
                (event, data)
            })
            .collect()
    }

    #[test]
    fn translates_text_stream_across_chunk_boundaries() {
        let mut translator = SseTranslator::new();
        let mut out = Vec::new();
        // Split mid-line to exercise the partial-line buffer.
        out.extend(translator.push(
            b"data: {\"id\":\"chat_1\",\"model\":\"m\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\ndata: {\"choices\":[{\"del",
        ));
        out.extend(translator.push(b"ta\":{\"content\":\"Hel\"}}]}\n\n"));
        out.extend(translator.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"));
        out.extend(
            translator.push(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"),
        );
        out.extend(translator.push(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n",
        ));
        out.extend(translator.push(b"data: [DONE]\n\n"));
        let events = events(&out);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        assert_eq!(events[0].1["message"]["id"], "chat_1");
        assert_eq!(events[0].1["message"]["model"], "m");
        assert_eq!(events[1].1["content_block"]["type"], "text");
        assert_eq!(events[2].1["delta"]["text"], "Hel");
        assert_eq!(events[3].1["delta"]["text"], "lo");
        assert_eq!(events[5].1["delta"]["stop_reason"], "end_turn");
        assert_eq!(events[5].1["usage"]["output_tokens"], 2);
        assert_eq!(events[5].1["usage"]["input_tokens"], 10);
    }

    #[test]
    fn translates_tool_call_stream() {
        let mut translator = SseTranslator::new();
        let mut out = Vec::new();
        out.extend(translator.push(
            b"data: {\"id\":\"chat_2\",\"model\":\"m\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
        ));
        out.extend(translator.push(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\"\"}}]}}]}\n\n",
        ));
        out.extend(translator.push(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"sh\\\"}\"}}]}}]}\n\n",
        ));
        out.extend(translator.push(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        ));
        out.extend(translator.push(b"data: [DONE]\n\n"));
        let events = events(&out);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        assert_eq!(events[1].1["content_block"]["type"], "tool_use");
        assert_eq!(events[1].1["content_block"]["id"], "call_1");
        assert_eq!(events[1].1["content_block"]["name"], "get_weather");
        let json_parts: String = events[2..4]
            .iter()
            .map(|(_, data)| data["delta"]["partial_json"].as_str().unwrap())
            .collect();
        assert_eq!(json_parts, "{\"city\":\"sh\"}");
        assert_eq!(events[5].1["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn flushes_unterminated_stream_on_finish() {
        let mut translator = SseTranslator::new();
        let mut out = Vec::new();
        out.extend(
            translator.push(b"data: {\"id\":\"chat_3\",\"model\":\"m\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"),
        );
        out.extend(translator.finish());
        let events = events(&out);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
    }
}
