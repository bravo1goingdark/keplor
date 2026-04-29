//! Input validation for [`IngestEvent`](crate::schema::IngestEvent).

use crate::error::ServerError;
use crate::schema::{IngestEvent, TimestampInput};

/// Maximum token count per field (10 million tokens per event is nonsensical).
const MAX_TOKENS: u32 = 10_000_000;
/// Maximum cost in nanodollars ($1,000 per call).
const MAX_COST_NANODOLLARS: i64 = 1_000_000_000_000;
/// Maximum string field length for attribution fields.
const MAX_ATTR_LEN: usize = 256;
/// Maximum endpoint field length.
const MAX_ENDPOINT_LEN: usize = 512;
/// Maximum metadata JSON size in bytes.
const MAX_METADATA_BYTES: usize = 65_536;
/// Minimum valid timestamp (2020-01-01T00:00:00Z in nanoseconds).
const MIN_TS_NS: i64 = 1_577_836_800_000_000_000;

/// Validate required fields and basic invariants.
pub fn validate(event: &IngestEvent) -> Result<(), ServerError> {
    // Required fields.
    if event.model.is_empty() {
        return Err(ServerError::Validation("model is required".into()));
    }
    if event.provider.is_empty() {
        return Err(ServerError::Validation("provider is required".into()));
    }
    if event.model.len() > 256 {
        return Err(ServerError::Validation("model exceeds 256 characters".into()));
    }
    if event.provider.len() > 128 {
        return Err(ServerError::Validation("provider exceeds 128 characters".into()));
    }

    // Token bounds.
    validate_tokens(&event.usage)?;

    // Cost bounds.
    if let Some(cost) = event.cost_nanodollars {
        if cost < 0 {
            return Err(ServerError::Validation("cost_nanodollars must not be negative".into()));
        }
        if cost > MAX_COST_NANODOLLARS {
            return Err(ServerError::Validation(format!(
                "cost_nanodollars {} exceeds maximum {MAX_COST_NANODOLLARS}",
                cost
            )));
        }
    }

    // Timestamp bounds.
    if let Some(TimestampInput::EpochNanos(ns)) = &event.timestamp {
        if *ns < MIN_TS_NS {
            return Err(ServerError::Validation("timestamp is before 2020-01-01".into()));
        }
        // Reject timestamps more than 24h in the future.
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let future_limit = now_ns + 86_400 * 1_000_000_000;
        if *ns > future_limit {
            return Err(ServerError::Validation(
                "timestamp is more than 24 hours in the future".into(),
            ));
        }
    }

    // Attribution string lengths.
    check_len(event.user_id.as_deref(), "user_id", MAX_ATTR_LEN)?;
    check_len(event.api_key_id.as_deref(), "api_key_id", MAX_ATTR_LEN)?;
    check_len(event.org_id.as_deref(), "org_id", MAX_ATTR_LEN)?;
    check_len(event.project_id.as_deref(), "project_id", MAX_ATTR_LEN)?;
    check_len(event.route_id.as_deref(), "route_id", MAX_ATTR_LEN)?;

    // Endpoint length.
    if event.endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(ServerError::Validation(format!(
            "endpoint exceeds {MAX_ENDPOINT_LEN} characters"
        )));
    }

    // Metadata size.
    if let Some(metadata) = &event.metadata {
        let size = serde_json::to_string(metadata).map(|s| s.len()).unwrap_or(0);
        if size > MAX_METADATA_BYTES {
            return Err(ServerError::Validation(format!(
                "metadata JSON exceeds {MAX_METADATA_BYTES} bytes ({size} bytes)"
            )));
        }
    }

    Ok(())
}

fn validate_tokens(u: &crate::schema::IngestUsage) -> Result<(), ServerError> {
    let fields = [
        ("input_tokens", u.input_tokens),
        ("output_tokens", u.output_tokens),
        ("cache_read_input_tokens", u.cache_read_input_tokens),
        ("cache_creation_input_tokens", u.cache_creation_input_tokens),
        ("reasoning_tokens", u.reasoning_tokens),
        ("audio_input_tokens", u.audio_input_tokens),
        ("audio_output_tokens", u.audio_output_tokens),
        ("image_tokens", u.image_tokens),
        ("tool_use_tokens", u.tool_use_tokens),
    ];
    for (name, val) in fields {
        if val > MAX_TOKENS {
            return Err(ServerError::Validation(format!(
                "{name} = {val} exceeds maximum {MAX_TOKENS}"
            )));
        }
    }
    Ok(())
}

fn check_len(value: Option<&str>, name: &str, max: usize) -> Result<(), ServerError> {
    if let Some(v) = value {
        if v.len() > max {
            return Err(ServerError::Validation(format!("{name} exceeds {max} characters")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> IngestEvent {
        serde_json::from_str(r#"{"model":"gpt-4o","provider":"openai"}"#).unwrap()
    }

    #[test]
    fn valid_minimal() {
        assert!(validate(&minimal()).is_ok());
    }

    #[test]
    fn rejects_empty_model() {
        let mut e = minimal();
        e.model = String::new();
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_empty_provider() {
        let mut e = minimal();
        e.provider = String::new();
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_excessive_tokens() {
        let mut e = minimal();
        e.usage.input_tokens = MAX_TOKENS + 1;
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_negative_cost() {
        let mut e = minimal();
        e.cost_nanodollars = Some(-1);
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_excessive_cost() {
        let mut e = minimal();
        e.cost_nanodollars = Some(MAX_COST_NANODOLLARS + 1);
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_ancient_timestamp() {
        let mut e = minimal();
        e.timestamp = Some(TimestampInput::EpochNanos(1_000_000_000_000_000));
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_far_future_timestamp() {
        let mut e = minimal();
        // Year 2100.
        e.timestamp = Some(TimestampInput::EpochNanos(4_102_444_800_000_000_000));
        assert!(validate(&e).is_err());
    }

    #[test]
    fn accepts_valid_recent_timestamp() {
        let mut e = minimal();
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        e.timestamp = Some(TimestampInput::EpochNanos(now_ns));
        assert!(validate(&e).is_ok());
    }

    #[test]
    fn rejects_long_user_id() {
        let mut e = minimal();
        e.user_id = Some("x".repeat(MAX_ATTR_LEN + 1));
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_long_endpoint() {
        let mut e = minimal();
        e.endpoint = "x".repeat(MAX_ENDPOINT_LEN + 1);
        assert!(validate(&e).is_err());
    }

    #[test]
    fn rejects_huge_metadata() {
        let mut e = minimal();
        let big = serde_json::Value::String("x".repeat(MAX_METADATA_BYTES + 1));
        e.metadata = Some(big);
        assert!(validate(&e).is_err());
    }

    #[test]
    fn accepts_valid_cost() {
        let mut e = minimal();
        e.cost_nanodollars = Some(1_000_000);
        assert!(validate(&e).is_ok());
    }

    #[test]
    fn unknown_field_rejected_at_deserialize() {
        // Wire-level: extras like request_body / response_body are
        // rejected by serde before the validator even runs.
        let json = r#"{"model":"gpt-4o","provider":"openai","request_body":{}}"#;
        let err = serde_json::from_str::<IngestEvent>(json).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }
}
