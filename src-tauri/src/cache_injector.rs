//! Prompt-cache breakpoint injector, ported from cc-switch.
//!
//! Anthropic Messages requests only benefit from prompt caching when the
//! caller marks `cache_control` breakpoints. Many clients never do, so the
//! gateway injects up to four ephemeral (5m) markers at the spots with the
//! best hit-rate: the tool list, the system prompt, the newest cacheable
//! message and one older user anchor for long tool loops. Existing caller
//! markers are always preserved and count against Anthropic's 4-breakpoint
//! budget.

use serde_json::{json, Value};

/// Inject cache breakpoints into an Anthropic Messages payload in place.
pub fn inject(body: &mut Value) {
    let existing = count_existing(body);
    let mut budget = 4_usize.saturating_sub(existing);
    if budget == 0 {
        // Either the caller already spent the whole budget or exceeded it;
        // both are caller-owned decisions the gateway must not rewrite.
        return;
    }

    // (a) End of the tool list: tools form the most stable prefix.
    if budget > 0 {
        if let Some(last) = body
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .and_then(|tools| tools.last_mut())
        {
            if mark(last) {
                budget -= 1;
            }
        }
    }

    // (b) End of the system prompt; string systems become one text block.
    if budget > 0 {
        if let Some(text) = body
            .get("system")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            body["system"] = json!([{ "type": "text", "text": text }]);
        }
        if let Some(last) = body
            .get_mut("system")
            .and_then(Value::as_array_mut)
            .and_then(|system| system.last_mut())
        {
            if mark(last) {
                budget -= 1;
            }
        }
    }

    // (c) Last non-thinking block of the newest cacheable message. Tool loops
    // usually end on user/tool_result turns; anchoring only assistant turns
    // would keep the newest stable prefix out of the cache.
    if budget > 0 {
        if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages.iter_mut().rev() {
                if mark_message(message) {
                    budget -= 1;
                    break;
                }
            }

            // (d) A second, older user anchor helps long tool-result turns
            // where the stable prefix falls outside Anthropic's 20-block
            // lookback from the newest breakpoint.
            if budget > 0 && messages.len() >= 4 {
                let mut user_count = 0;
                for message in messages.iter_mut().rev() {
                    if message.get("role").and_then(Value::as_str) != Some("user") {
                        continue;
                    }
                    user_count += 1;
                    if user_count == 2 {
                        mark_message(message);
                        break;
                    }
                }
            }
        }
    }
}

/// Mark the last non-thinking content block of a message. Returns false when
/// the message has no block worth marking or is already marked.
fn mark_message(message: &mut Value) -> bool {
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(block) = content.iter_mut().rev().find(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("thinking" | "redacted_thinking")
        )
    }) else {
        return false;
    };
    mark(block)
}

/// Attach a standard 5-minute ephemeral marker unless the block has one.
fn mark(block: &mut Value) -> bool {
    if block.get("cache_control").is_some() {
        return false;
    }
    let Some(object) = block.as_object_mut() else {
        return false;
    };
    object.insert("cache_control".into(), json!({ "type": "ephemeral" }));
    true
}

fn count_existing(body: &Value) -> usize {
    let block_count = |value: Option<&Value>| {
        value
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block.get("cache_control").is_some())
                    .count()
            })
            .unwrap_or(0)
    };
    let mut count = block_count(body.get("tools")) + block_count(body.get("system"));
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            count += block_count(message.get("content"));
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_tools_system_and_latest_message() {
        let mut body = json!({
            "model": "claude",
            "tools": [{"name": "tool1"}, {"name": "tool2"}],
            "system": [{"type": "text", "text": "sys prompt"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}
            ]
        });
        inject(&mut body);
        assert!(body["tools"][1].get("cache_control").is_some());
        assert!(body["tools"][0].get("cache_control").is_none());
        assert!(body["system"][0].get("cache_control").is_some());
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
        assert_eq!(
            body["tools"][1]["cache_control"],
            json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn converts_string_system_to_block_array() {
        let mut body = json!({
            "system": "You are a helpful assistant",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });
        inject(&mut body);
        assert_eq!(body["system"][0]["text"], "You are a helpful assistant");
        assert!(body["system"][0].get("cache_control").is_some());
    }

    #[test]
    fn long_history_adds_prior_user_anchor() {
        let mut body = json!({
            "tools": [{"name": "tool1"}],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "first"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "answer"}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "c1", "content": "result"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "latest"}]}
            ]
        });
        inject(&mut body);
        assert_eq!(count_existing(&body), 4);
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn respects_existing_breakpoints_and_budget() {
        let mut body = json!({
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral", "ttl": "1h"}},
                {"name": "t2", "cache_control": {"type": "ephemeral", "ttl": "1h"}}
            ],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}
            ]
        });
        inject(&mut body);
        // Caller markers keep their TTL; budget 4-2=2 goes to system + message.
        assert_eq!(body["tools"][0]["cache_control"]["ttl"], "1h");
        assert!(body["system"][0].get("cache_control").is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
        assert_eq!(count_existing(&body), 4);
    }

    #[test]
    fn full_budget_is_a_no_op() {
        let mut body = json!({
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral"}},
                {"name": "t2", "cache_control": {"type": "ephemeral"}}
            ],
            "system": [
                {"type": "text", "text": "s1", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "s2", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "m1"}]}]
        });
        let original = body.clone();
        inject(&mut body);
        assert_eq!(body, original);
    }

    #[test]
    fn skips_thinking_blocks() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm"},
                    {"type": "text", "text": "result"},
                    {"type": "redacted_thinking", "data": "xxx"}
                ]}
            ]
        });
        inject(&mut body);
        assert!(body["messages"][0]["content"][1]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][0]["content"][2]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn anchors_latest_tool_result_over_older_assistant() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "call_1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "done"}]}
            ]
        });
        inject(&mut body);
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn ignores_non_object_payloads() {
        let mut body = Value::Null;
        inject(&mut body);
        assert_eq!(body, Value::Null);
        let mut body = json!({"messages": "not-an-array"});
        inject(&mut body);
        assert_eq!(body, json!({"messages": "not-an-array"}));
    }
}
