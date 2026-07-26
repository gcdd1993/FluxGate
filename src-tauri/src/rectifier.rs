use crate::models::RectifierConfig;
use serde_json::{json, Value};

const UNSUPPORTED_IMAGE: &str = "[Unsupported Image]";

pub fn preflight(payload: &Value, model: &str, config: &RectifierConfig) -> Value {
    let mut next = payload.clone();
    if config.enabled
        && config.media_fallback
        && config.text_only_preflight
        && is_confirmed_text_only(model)
    {
        replace_images(&mut next);
    }
    next
}

pub fn rectify_for_error(
    payload: &Value,
    error_message: &str,
    config: &RectifierConfig,
) -> Option<Value> {
    if !config.enabled {
        return None;
    }
    let lower = error_message.to_lowercase();
    let mut next = payload.clone();
    let mut applied = false;

    if config.thinking_signature && is_thinking_compatibility_error(&lower) {
        applied |= remove_unsupported_thinking(&mut next, &lower);
    }

    if config.thinking_budget && is_thinking_budget_error(&lower) {
        applied |= rectify_thinking_budget(&mut next);
    }

    if config.media_fallback && is_media_error(&lower) {
        applied |= replace_images(&mut next);
    }

    applied.then_some(next)
}

fn is_thinking_compatibility_error(error: &str) -> bool {
    let mentions_thinking = error.contains("thinking")
        || error.contains("redacted_thinking")
        || error.contains("thought signature")
        || error.contains("reasoning_effort");
    let incompatible = error.contains("unsupported parameter")
        || error.contains("unknown parameter")
        || error.contains("unrecognized parameter")
        || error.contains("extra inputs are not permitted")
        || error.contains("invalid")
        || error.contains("not valid")
        || error.contains("field required")
        || error.contains("must start with a thinking block")
        || error.contains("cannot be modified")
        || error.contains("非法请求")
        || error.contains("illegal request");
    mentions_thinking && incompatible
}

fn remove_unsupported_thinking(value: &mut Value, error: &str) -> bool {
    let mut changed = false;
    if let Some(object) = value.as_object_mut() {
        if error.contains("thinking") {
            changed |= object.remove("thinking").is_some();
        }
        if error.contains("reasoning_effort") {
            changed |= object.remove("reasoning_effort").is_some();
        }
        if error.contains("reasoning") && !error.contains("reasoning_effort") {
            changed |= object.remove("reasoning").is_some();
        }
        changed |= object.remove("signature").is_some();
        for child in object.values_mut() {
            changed |= remove_thinking_children(child);
        }
    }
    changed
}

fn remove_thinking_children(value: &mut Value) -> bool {
    let mut changed = false;
    match value {
        Value::Array(items) => {
            let before = items.len();
            items.retain(|item| {
                !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            });
            changed |= items.len() != before;
            for item in items {
                changed |= remove_thinking_children(item);
            }
        }
        Value::Object(object) => {
            changed |= object.remove("signature").is_some();
            for child in object.values_mut() {
                changed |= remove_thinking_children(child);
            }
        }
        _ => {}
    }
    changed
}

fn is_thinking_budget_error(error: &str) -> bool {
    let budget = error.contains("budget_tokens") || error.contains("budget tokens");
    let minimum = error.contains("greater than or equal to 1024")
        || error.contains(">= 1024")
        || (error.contains("1024") && error.contains("input should be"));
    budget && error.contains("thinking") && minimum
}

fn rectify_thinking_budget(value: &mut Value) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    if object
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        == Some("adaptive")
    {
        return false;
    }
    let before = object.clone();
    object.insert(
        "thinking".into(),
        json!({"type":"enabled","budget_tokens":32000}),
    );
    if object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        < 32001
    {
        object.insert("max_tokens".into(), json!(64000));
    }
    *object != before
}

fn is_media_error(error: &str) -> bool {
    let media = error.contains("image")
        || error.contains("vision")
        || error.contains("media")
        || error.contains("only support text")
        || error.contains("only supports text");
    let rejected = error.contains("unsupported")
        || error.contains("not support")
        || error.contains("does not support")
        || error.contains("unknown variant")
        || error.contains("invalid")
        || error.contains("only support text")
        || error.contains("only supports text");
    media && rejected
}

fn replace_images(value: &mut Value) -> bool {
    match value {
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= replace_images(item);
            }
            changed
        }
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str);
            if matches!(kind, Some("image" | "image_url" | "input_image")) {
                *value = if kind == Some("input_image") {
                    json!({"type":"input_text","text":UNSUPPORTED_IMAGE})
                } else {
                    json!({"type":"text","text":UNSUPPORTED_IMAGE})
                };
                return true;
            }
            let mut changed = false;
            for child in object.values_mut() {
                changed |= replace_images(child);
            }
            changed
        }
        _ => false,
    }
}

fn is_confirmed_text_only(model: &str) -> bool {
    let tail = model
        .rsplit_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(model)
        .to_ascii_lowercase();
    matches!(
        tail.as_str(),
        "deepseek-v4-flash" | "deepseek-v4-pro" | "glm-5.2" | "longcat-2.0"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_workbuddy_thinking_parameter() {
        let body = json!({"model":"deepseek-v4-flash","thinking":{"type":"enabled"},"messages":[]});
        let fixed = rectify_for_error(
            &body,
            "Validation: Unsupported parameter(s): `thinking`",
            &RectifierConfig::default(),
        )
        .unwrap();
        assert!(fixed.get("thinking").is_none());
    }

    #[test]
    fn repairs_thinking_budget_like_cc_switch() {
        let body = json!({"thinking":{"type":"enabled","budget_tokens":512},"max_tokens":1024});
        let fixed = rectify_for_error(
            &body,
            "thinking.budget_tokens: Input should be greater than or equal to 1024",
            &RectifierConfig::default(),
        )
        .unwrap();
        assert_eq!(fixed["thinking"]["budget_tokens"], 32000);
        assert_eq!(fixed["max_tokens"], 64000);
    }

    #[test]
    fn text_only_preflight_replaces_openai_images() {
        let body = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,x"}}]}]});
        let fixed = preflight(
            &body,
            "deepseek-ai/deepseek-v4-flash",
            &RectifierConfig::default(),
        );
        assert_eq!(
            fixed["messages"][0]["content"][0]["text"],
            UNSUPPORTED_IMAGE
        );
    }
}
