//! Provider and model normalisation.

use keplor_core::Provider;
use smol_str::SmolStr;

/// Map a provider string from the ingestion schema to a [`Provider`].
///
/// Case-insensitive, zero-allocation for the 12 known providers.
#[inline]
pub fn normalize_provider(raw: &str) -> Provider {
    Provider::from_id_key_ignore_case(raw)
}

/// Normalise a model name: trim whitespace, lowercase for catalog lookup.
#[inline]
pub fn normalize_model(raw: &str) -> SmolStr {
    SmolStr::new(raw.trim().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_providers_case_insensitive() {
        assert_eq!(normalize_provider("OpenAI"), Provider::OpenAI);
        assert_eq!(normalize_provider("ANTHROPIC"), Provider::Anthropic);
        assert_eq!(normalize_provider("gemini"), Provider::Gemini);
    }

    #[test]
    fn unknown_becomes_compatible() {
        let p = normalize_provider("custom-proxy");
        assert!(matches!(p, Provider::OpenAICompatible { .. }));
    }

    #[test]
    fn model_trimmed_and_lowered() {
        assert_eq!(normalize_model("  GPT-4o  "), SmolStr::new("gpt-4o"));
    }
}
