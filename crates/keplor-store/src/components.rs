//! Payload-component splitting: extract dedup-friendly pieces from
//! request/response JSON bodies.

use bytes::Bytes;
use keplor_core::Provider;

/// Identifies what kind of component a blob represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentType {
    /// The system-level instruction (OpenAI `messages[role=system]`,
    /// Anthropic `system` field).
    SystemPrompt,
    /// Tool / function schema array.
    Tools,
    /// The conversation messages (everything except system + tools).
    Messages,
    /// Response body.
    Response,
    /// Unsplit raw body (fallback when JSON parsing fails).
    Raw,
}

impl ComponentType {
    /// Stable string key for storage.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::Tools => "tools",
            Self::Messages => "messages",
            Self::Response => "response",
            Self::Raw => "raw",
        }
    }
}

/// A single extracted component.
#[derive(Debug, Clone)]
pub struct Component {
    /// What kind of component this is.
    pub kind: ComponentType,
    /// The raw bytes of this component.
    pub data: Bytes,
}

/// Split a request body into dedup-friendly components.
///
/// Returns at least one component.  If JSON parsing fails, returns a
/// single [`ComponentType::Raw`] component containing the full body.
pub fn split_request(provider: &Provider, body: &Bytes) -> Vec<Component> {
    if body.is_empty() {
        return vec![Component { kind: ComponentType::Raw, data: body.clone() }];
    }

    let parsed = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) => v,
        Err(_) => {
            return vec![Component { kind: ComponentType::Raw, data: body.clone() }];
        },
    };

    let mut components = Vec::with_capacity(3);

    match provider {
        Provider::Anthropic | Provider::Bedrock => {
            extract_anthropic_request(&parsed, &mut components);
        },
        _ => {
            extract_openai_request(&parsed, &mut components);
        },
    }

    if components.is_empty() {
        components.push(Component { kind: ComponentType::Raw, data: body.clone() });
    }

    components
}

/// Split a response body into components (always a single response component).
pub fn split_response(body: &Bytes) -> Vec<Component> {
    vec![Component { kind: ComponentType::Response, data: body.clone() }]
}

fn serialize_to_bytes(value: &serde_json::Value) -> Option<Bytes> {
    let mut buf = Vec::with_capacity(128);
    serde_json::to_writer(&mut buf, value).ok()?;
    Some(Bytes::from(buf))
}

fn extract_openai_request(parsed: &serde_json::Value, out: &mut Vec<Component>) {
    if let Some(messages) = parsed.get("messages").and_then(|v| v.as_array()) {
        let mut sys_buf = Vec::with_capacity(128);
        let mut msg_buf = Vec::with_capacity(128);
        let mut has_system = false;
        let mut has_other = false;

        sys_buf.push(b'[');
        msg_buf.push(b'[');

        for m in messages {
            let is_system = m.get("role").and_then(|r| r.as_str()) == Some("system");
            let buf = if is_system {
                has_system = true;
                &mut sys_buf
            } else {
                has_other = true;
                &mut msg_buf
            };
            if buf.len() > 1 {
                buf.push(b',');
            }
            // write directly into the buffer without intermediate allocation
            let _ = serde_json::to_writer(&mut *buf, m);
        }

        sys_buf.push(b']');
        msg_buf.push(b']');

        if has_system {
            out.push(Component { kind: ComponentType::SystemPrompt, data: Bytes::from(sys_buf) });
        }
        if has_other {
            out.push(Component { kind: ComponentType::Messages, data: Bytes::from(msg_buf) });
        }
    }

    if let Some(tools) = parsed.get("tools") {
        if let Some(bytes) = serialize_to_bytes(tools) {
            out.push(Component { kind: ComponentType::Tools, data: bytes });
        }
    } else if let Some(functions) = parsed.get("functions") {
        if let Some(bytes) = serialize_to_bytes(functions) {
            out.push(Component { kind: ComponentType::Tools, data: bytes });
        }
    }
}

fn extract_anthropic_request(parsed: &serde_json::Value, out: &mut Vec<Component>) {
    if let Some(system) = parsed.get("system") {
        if let Some(bytes) = serialize_to_bytes(system) {
            out.push(Component { kind: ComponentType::SystemPrompt, data: bytes });
        }
    }

    if let Some(messages) = parsed.get("messages") {
        if let Some(bytes) = serialize_to_bytes(messages) {
            out.push(Component { kind: ComponentType::Messages, data: bytes });
        }
    }

    if let Some(tools) = parsed.get("tools") {
        if let Some(bytes) = serialize_to_bytes(tools) {
            out.push(Component { kind: ComponentType::Tools, data: bytes });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_request_splits_system_and_messages() {
        let body = Bytes::from(
            r#"{
                "model": "gpt-4o",
                "messages": [
                    {"role": "system", "content": "You are a helpful assistant."},
                    {"role": "user", "content": "Hello!"}
                ]
            }"#,
        );
        let components = split_request(&Provider::OpenAI, &body);
        assert!(components.iter().any(|c| c.kind == ComponentType::SystemPrompt));
        assert!(components.iter().any(|c| c.kind == ComponentType::Messages));
    }

    #[test]
    fn openai_request_with_tools() {
        let body = Bytes::from(
            r#"{
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "Hi"}],
                "tools": [{"type": "function", "function": {"name": "get_weather"}}]
            }"#,
        );
        let components = split_request(&Provider::OpenAI, &body);
        assert!(components.iter().any(|c| c.kind == ComponentType::Tools));
    }

    #[test]
    fn anthropic_request_splits_system() {
        let body = Bytes::from(
            r#"{
                "model": "claude-sonnet-4-20250514",
                "system": "You are a helpful assistant.",
                "messages": [{"role": "user", "content": "Hello!"}]
            }"#,
        );
        let components = split_request(&Provider::Anthropic, &body);
        assert!(components.iter().any(|c| c.kind == ComponentType::SystemPrompt));
        assert!(components.iter().any(|c| c.kind == ComponentType::Messages));
    }

    #[test]
    fn non_json_body_returns_raw() {
        let body = Bytes::from("not json at all");
        let components = split_request(&Provider::OpenAI, &body);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].kind, ComponentType::Raw);
    }

    #[test]
    fn empty_body_returns_raw() {
        let components = split_request(&Provider::OpenAI, &Bytes::new());
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].kind, ComponentType::Raw);
    }

    #[test]
    fn response_is_single_component() {
        let body = Bytes::from(r#"{"id":"chatcmpl-abc","choices":[{"message":{"content":"Hi"}}]}"#);
        let components = split_response(&body);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].kind, ComponentType::Response);
    }

    #[test]
    fn system_prompt_dedup_same_bytes() {
        let body1 = Bytes::from(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"A"}]}"#,
        );
        let body2 = Bytes::from(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"B"}]}"#,
        );
        let c1 = split_request(&Provider::OpenAI, &body1);
        let c2 = split_request(&Provider::OpenAI, &body2);

        let sp1 = c1.iter().find(|c| c.kind == ComponentType::SystemPrompt).unwrap();
        let sp2 = c2.iter().find(|c| c.kind == ComponentType::SystemPrompt).unwrap();
        assert_eq!(sp1.data, sp2.data, "same system prompt should produce identical bytes");

        let m1 = c1.iter().find(|c| c.kind == ComponentType::Messages).unwrap();
        let m2 = c2.iter().find(|c| c.kind == ComponentType::Messages).unwrap();
        assert_ne!(m1.data, m2.data, "different user messages should differ");
    }
}
