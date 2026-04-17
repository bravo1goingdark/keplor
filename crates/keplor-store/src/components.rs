//! Payload-component splitting: extract dedup-friendly pieces from
//! request/response JSON bodies.
//!
//! Uses `serde_json::value::RawValue` to avoid building a full
//! heap-allocated `Value` tree.  The input bytes are already canonical
//! serde_json output (produced by `pipeline.rs`), so raw byte slicing
//! preserves dedup-safe determinism.

use bytes::Bytes;
use keplor_core::Provider;
use serde::Deserialize;
use serde_json::value::RawValue;

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

    let mut components = Vec::with_capacity(3);

    let ok = match provider {
        Provider::Anthropic | Provider::Bedrock => extract_anthropic_request(body, &mut components),
        _ => extract_openai_request(body, &mut components),
    };

    if !ok || components.is_empty() {
        components.clear();
        components.push(Component { kind: ComponentType::Raw, data: body.clone() });
    }

    components
}

/// Split a response body into components (always a single response component).
pub fn split_response(body: &Bytes) -> Vec<Component> {
    vec![Component { kind: ComponentType::Response, data: body.clone() }]
}

// ── OpenAI extraction ──────────────────────────────────────────────────

/// Minimal top-level struct — borrows field values as raw byte slices
/// instead of allocating a full `Value` tree.
#[derive(Deserialize)]
struct OpenAIShell<'a> {
    #[serde(borrow)]
    messages: Option<Vec<&'a RawValue>>,
    #[serde(borrow)]
    tools: Option<&'a RawValue>,
    #[serde(borrow)]
    functions: Option<&'a RawValue>,
}

/// Just enough to check the `role` field of a message.
#[derive(Deserialize)]
struct RoleCheck<'a> {
    #[serde(borrow)]
    role: Option<&'a str>,
}

fn extract_openai_request(body: &[u8], out: &mut Vec<Component>) -> bool {
    let shell: OpenAIShell<'_> = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(_) => return false,
    };

    if let Some(messages) = &shell.messages {
        let mut sys_buf = Vec::with_capacity(128);
        let mut msg_buf = Vec::with_capacity(128);
        let mut has_system = false;
        let mut has_other = false;

        sys_buf.push(b'[');
        msg_buf.push(b'[');

        for msg_raw in messages {
            let is_system = match serde_json::from_str::<RoleCheck<'_>>(msg_raw.get()) {
                Ok(rc) => rc.role == Some("system"),
                Err(_) => false,
            };

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
            // Raw bytes from RawValue are already canonical serde_json
            // output — copy them directly, no re-serialization needed.
            buf.extend_from_slice(msg_raw.get().as_bytes());
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

    if let Some(tools) = &shell.tools {
        out.push(Component {
            kind: ComponentType::Tools,
            data: Bytes::copy_from_slice(tools.get().as_bytes()),
        });
    } else if let Some(functions) = &shell.functions {
        out.push(Component {
            kind: ComponentType::Tools,
            data: Bytes::copy_from_slice(functions.get().as_bytes()),
        });
    }

    true
}

// ── Anthropic extraction ───────────────────────────────────────────────

#[derive(Deserialize)]
struct AnthropicShell<'a> {
    #[serde(borrow)]
    system: Option<&'a RawValue>,
    #[serde(borrow)]
    messages: Option<&'a RawValue>,
    #[serde(borrow)]
    tools: Option<&'a RawValue>,
}

fn extract_anthropic_request(body: &[u8], out: &mut Vec<Component>) -> bool {
    let shell: AnthropicShell<'_> = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(_) => return false,
    };

    if let Some(system) = &shell.system {
        out.push(Component {
            kind: ComponentType::SystemPrompt,
            data: Bytes::copy_from_slice(system.get().as_bytes()),
        });
    }

    if let Some(messages) = &shell.messages {
        out.push(Component {
            kind: ComponentType::Messages,
            data: Bytes::copy_from_slice(messages.get().as_bytes()),
        });
    }

    if let Some(tools) = &shell.tools {
        out.push(Component {
            kind: ComponentType::Tools,
            data: Bytes::copy_from_slice(tools.get().as_bytes()),
        });
    }

    true
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
    fn system_prompt_dedup_canonical_bytes() {
        // Simulate pipeline canonical output: serde_json::to_vec of a Value.
        let v1: serde_json::Value = serde_json::from_str(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"A"}]}"#,
        )
        .unwrap();
        let v2: serde_json::Value = serde_json::from_str(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"B"}]}"#,
        )
        .unwrap();
        let body1 = Bytes::from(serde_json::to_vec(&v1).unwrap());
        let body2 = Bytes::from(serde_json::to_vec(&v2).unwrap());

        let c1 = split_request(&Provider::OpenAI, &body1);
        let c2 = split_request(&Provider::OpenAI, &body2);

        let sp1 = c1.iter().find(|c| c.kind == ComponentType::SystemPrompt).unwrap();
        let sp2 = c2.iter().find(|c| c.kind == ComponentType::SystemPrompt).unwrap();
        assert_eq!(sp1.data, sp2.data, "same system prompt should produce identical bytes");

        let m1 = c1.iter().find(|c| c.kind == ComponentType::Messages).unwrap();
        let m2 = c2.iter().find(|c| c.kind == ComponentType::Messages).unwrap();
        assert_ne!(m1.data, m2.data, "different user messages should differ");
    }

    #[test]
    fn canonical_bytes_match_old_format() {
        // Verify that RawValue-based extraction produces the same bytes
        // as the old Value-based approach for canonical serde_json input.
        let v: serde_json::Value = serde_json::from_str(
            r#"{"model":"gpt-4o","messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"Hello!"}],"tools":[{"type":"function"}]}"#,
        )
        .unwrap();
        let canonical = Bytes::from(serde_json::to_vec(&v).unwrap());

        let components = split_request(&Provider::OpenAI, &canonical);
        assert_eq!(components.len(), 3);

        let sys = components.iter().find(|c| c.kind == ComponentType::SystemPrompt).unwrap();
        let msgs = components.iter().find(|c| c.kind == ComponentType::Messages).unwrap();
        let tools = components.iter().find(|c| c.kind == ComponentType::Tools).unwrap();

        // System prompt: single message wrapped in array
        let expected_sys: serde_json::Value =
            serde_json::from_str(r#"[{"role":"system","content":"Be helpful."}]"#).unwrap();
        let expected_sys_bytes = serde_json::to_vec(&expected_sys).unwrap();
        assert_eq!(sys.data.as_ref(), expected_sys_bytes.as_slice());

        // Messages: non-system messages wrapped in array
        let expected_msgs: serde_json::Value =
            serde_json::from_str(r#"[{"role":"user","content":"Hello!"}]"#).unwrap();
        let expected_msgs_bytes = serde_json::to_vec(&expected_msgs).unwrap();
        assert_eq!(msgs.data.as_ref(), expected_msgs_bytes.as_slice());

        // Tools: raw value
        let expected_tools: serde_json::Value =
            serde_json::from_str(r#"[{"type":"function"}]"#).unwrap();
        let expected_tools_bytes = serde_json::to_vec(&expected_tools).unwrap();
        assert_eq!(tools.data.as_ref(), expected_tools_bytes.as_slice());
    }
}
