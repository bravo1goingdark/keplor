//! Input validation for [`IngestEvent`](crate::schema::IngestEvent).

use crate::error::ServerError;
use crate::schema::IngestEvent;

/// Validate required fields and basic invariants.
pub fn validate(event: &IngestEvent) -> Result<(), ServerError> {
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
}
