//! The [`Provider`] enum: what LLM vendor / surface an event targets.
//!
//! Keplor is an ingestion server — it receives pre-parsed events from
//! external systems.  This enum identifies which provider the event
//! came from and is used for four things only:
//!
//! 1. Looking up pricing rows.
//! 2. Attributing cost / usage for dashboards.
//! 3. Selecting the right compression dictionary.
//! 4. Picking the right auth header when we later add server-side virtual
//!    keys (phase 10).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The 16 provider surfaces Keplor understands.
///
/// Keep variant order stable — the [`Serialize`] impl uses variant names as
/// stable storage keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provider {
    /// `api.openai.com` (Chat Completions + Responses).
    OpenAI,
    /// `api.anthropic.com` Messages.
    Anthropic,
    /// Public AI Studio (`generativelanguage.googleapis.com`).
    Gemini,
    /// Vertex AI (`*-aiplatform.googleapis.com`).
    GeminiVertex,
    /// AWS Bedrock runtime (any `bedrock-runtime.{region}.amazonaws.com`).
    Bedrock,
    /// Azure-hosted OpenAI (`*.openai.azure.com`).
    AzureOpenAI,
    /// `api.mistral.ai`.
    Mistral,
    /// `api.groq.com`.
    Groq,
    /// xAI Grok (`api.x.ai`).
    XAi,
    /// DeepSeek (`api.deepseek.com`).
    DeepSeek,
    /// Cohere v2 (`api.cohere.com`).
    Cohere,
    /// OpenRouter (`openrouter.ai`).
    OpenRouter,
    /// Local Ollama (`localhost:11434` by default).
    Ollama,
    /// OpenCode Go subscription gateway (`opencode.ai/zen/go`).
    /// Flat-rate $10/mo plan reselling curated open-source coding models
    /// (GLM, Kimi, MiniMax, MiMo, …) over an OpenAI-compatible API.
    OpenCode,
    /// OpenCode Zen pay-as-you-go gateway (`opencode.ai/zen`).
    /// Same wire format as OpenCode Go but charges per token; ships
    /// frontier models (Claude, GPT, Gemini) alongside the Go set.
    OpenCodeZen,
    /// Any other base URL that speaks the OpenAI Chat Completions dialect.
    OpenAICompatible {
        /// Base URL of the compatible endpoint (including scheme + host).
        base_url: Arc<str>,
    },
}

impl Provider {
    /// Canonical host string for logs and provider-id tagging.  For
    /// [`Provider::OpenAICompatible`] the returned slice is the base URL
    /// verbatim.
    #[must_use]
    pub fn canonical_host(&self) -> &str {
        match self {
            Self::OpenAI => "api.openai.com",
            Self::Anthropic => "api.anthropic.com",
            Self::Gemini => "generativelanguage.googleapis.com",
            Self::GeminiVertex => "aiplatform.googleapis.com",
            Self::Bedrock => "bedrock-runtime.amazonaws.com",
            Self::AzureOpenAI => "openai.azure.com",
            Self::Mistral => "api.mistral.ai",
            Self::Groq => "api.groq.com",
            Self::XAi => "api.x.ai",
            Self::DeepSeek => "api.deepseek.com",
            Self::Cohere => "api.cohere.com",
            Self::OpenRouter => "openrouter.ai",
            Self::Ollama => "localhost",
            Self::OpenCode => "opencode.ai",
            Self::OpenCodeZen => "opencode.ai",
            Self::OpenAICompatible { base_url } => base_url,
        }
    }

    /// Stable string key for the provider — lowercase, no punctuation.
    /// Used as the serialisation discriminator and for storage / metrics
    /// labels.  Returns `&'static str` for all known providers, enabling
    /// zero-allocation metrics label usage.
    #[must_use]
    pub fn id_key(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::GeminiVertex => "gemini_vertex",
            Self::Bedrock => "bedrock",
            Self::AzureOpenAI => "azure_openai",
            Self::Mistral => "mistral",
            Self::Groq => "groq",
            Self::XAi => "xai",
            Self::DeepSeek => "deepseek",
            Self::Cohere => "cohere",
            Self::OpenRouter => "openrouter",
            Self::Ollama => "ollama",
            Self::OpenCode => "opencode",
            Self::OpenCodeZen => "opencode_zen",
            Self::OpenAICompatible { .. } => "openai_compatible",
        }
    }

    /// Name of the HTTP header the provider uses for authentication.
    ///
    /// This is the *header name* only — the *value* format is provider
    /// specific (`Bearer …`, raw key, SigV4 signature, etc.).
    #[must_use]
    pub fn auth_header_name(&self) -> &str {
        match self {
            Self::Anthropic => "x-api-key",
            Self::Gemini => "x-goog-api-key",
            Self::AzureOpenAI => "api-key",
            // Vertex, Bedrock, and everything OpenAI-compatible use
            // bearer-style `Authorization`.  Bedrock signs with AWS SigV4
            // but the header is still `authorization`.
            _ => "authorization",
        }
    }

    /// Parse a provider from its stable `id_key` string (exact match).
    ///
    /// Unknown strings become [`Provider::OpenAICompatible`].
    #[must_use]
    pub fn from_id_key(s: &str) -> Self {
        match s {
            "openai" => Self::OpenAI,
            "anthropic" => Self::Anthropic,
            "gemini" => Self::Gemini,
            "gemini_vertex" => Self::GeminiVertex,
            "bedrock" => Self::Bedrock,
            "azure_openai" => Self::AzureOpenAI,
            "mistral" => Self::Mistral,
            "groq" => Self::Groq,
            "xai" => Self::XAi,
            "deepseek" => Self::DeepSeek,
            "cohere" => Self::Cohere,
            "openrouter" => Self::OpenRouter,
            "ollama" => Self::Ollama,
            "opencode" => Self::OpenCode,
            "opencode_zen" => Self::OpenCodeZen,
            other => Self::OpenAICompatible { base_url: Arc::from(other) },
        }
    }

    /// Case-insensitive parse without allocating a lowercase copy.
    #[must_use]
    pub fn from_id_key_ignore_case(s: &str) -> Self {
        if s.eq_ignore_ascii_case("openai") {
            Self::OpenAI
        } else if s.eq_ignore_ascii_case("anthropic") {
            Self::Anthropic
        } else if s.eq_ignore_ascii_case("gemini") {
            Self::Gemini
        } else if s.eq_ignore_ascii_case("gemini_vertex") {
            Self::GeminiVertex
        } else if s.eq_ignore_ascii_case("bedrock") {
            Self::Bedrock
        } else if s.eq_ignore_ascii_case("azure_openai") {
            Self::AzureOpenAI
        } else if s.eq_ignore_ascii_case("mistral") {
            Self::Mistral
        } else if s.eq_ignore_ascii_case("groq") {
            Self::Groq
        } else if s.eq_ignore_ascii_case("xai") {
            Self::XAi
        } else if s.eq_ignore_ascii_case("deepseek") {
            Self::DeepSeek
        } else if s.eq_ignore_ascii_case("cohere") {
            Self::Cohere
        } else if s.eq_ignore_ascii_case("openrouter") {
            Self::OpenRouter
        } else if s.eq_ignore_ascii_case("ollama") {
            Self::Ollama
        } else if s.eq_ignore_ascii_case("opencode") {
            Self::OpenCode
        } else if s.eq_ignore_ascii_case("opencode_zen") || s.eq_ignore_ascii_case("opencode-zen") {
            Self::OpenCodeZen
        } else {
            Self::OpenAICompatible { base_url: Arc::from(s) }
        }
    }

    /// Best-effort classification of an incoming request by `Host` header
    /// and URL path.
    ///
    /// Returns `None` if nothing matches — callers should fall back to
    /// [`Provider::OpenAICompatible`] once they know the base URL.
    #[must_use]
    pub fn from_host_path(host: &str, path: &str) -> Option<Self> {
        let h = host.to_ascii_lowercase();

        if h == "api.openai.com" {
            return Some(Self::OpenAI);
        }
        if h == "api.anthropic.com" {
            return Some(Self::Anthropic);
        }
        if h == "generativelanguage.googleapis.com" {
            return Some(Self::Gemini);
        }
        if h.ends_with("aiplatform.googleapis.com") {
            return Some(Self::GeminiVertex);
        }
        if h.starts_with("bedrock-runtime.") && h.ends_with(".amazonaws.com") {
            return Some(Self::Bedrock);
        }
        if h.ends_with(".openai.azure.com") {
            return Some(Self::AzureOpenAI);
        }
        if h == "api.mistral.ai" {
            return Some(Self::Mistral);
        }
        if h == "api.groq.com" {
            return Some(Self::Groq);
        }
        if h == "api.x.ai" {
            return Some(Self::XAi);
        }
        if h == "api.deepseek.com" {
            return Some(Self::DeepSeek);
        }
        if h == "api.cohere.com" || h == "api.cohere.ai" {
            return Some(Self::Cohere);
        }
        if h == "openrouter.ai" {
            return Some(Self::OpenRouter);
        }
        // OpenCode Go and Zen share the same host. Disambiguate by path
        // prefix — `/zen/go/...` is the Go subscription, `/zen/...` (no
        // `/go`) is the Zen pay-as-you-go gateway.
        if h == "opencode.ai" {
            if path.starts_with("/zen/go") {
                return Some(Self::OpenCode);
            }
            if path.starts_with("/zen") {
                return Some(Self::OpenCodeZen);
            }
        }
        if h == "localhost" || h.starts_with("localhost:") || h == "127.0.0.1" {
            // Default Ollama port is 11434; also accept arbitrary local
            // ports that talk to /api/chat or /api/generate.
            if path.starts_with("/api/") || h.ends_with(":11434") {
                return Some(Self::Ollama);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_matching_battery() {
        let cases: &[(&str, &str, Option<Provider>)] = &[
            ("api.openai.com", "/v1/chat/completions", Some(Provider::OpenAI)),
            ("API.OpenAI.com", "/v1/responses", Some(Provider::OpenAI)),
            ("api.anthropic.com", "/v1/messages", Some(Provider::Anthropic)),
            (
                "generativelanguage.googleapis.com",
                "/v1beta/models/gemini-pro:streamGenerateContent",
                Some(Provider::Gemini),
            ),
            (
                "us-central1-aiplatform.googleapis.com",
                "/v1/projects/x/locations/us-central1/publishers/google/models/gemini-pro:streamGenerateContent",
                Some(Provider::GeminiVertex),
            ),
            (
                "bedrock-runtime.us-east-1.amazonaws.com",
                "/model/anthropic.claude-3-sonnet/converse-stream",
                Some(Provider::Bedrock),
            ),
            (
                "my-resource.openai.azure.com",
                "/openai/deployments/gpt-4/chat/completions?api-version=2024-02-01",
                Some(Provider::AzureOpenAI),
            ),
            ("api.mistral.ai", "/v1/chat/completions", Some(Provider::Mistral)),
            ("api.groq.com", "/openai/v1/chat/completions", Some(Provider::Groq)),
            ("api.x.ai", "/v1/chat/completions", Some(Provider::XAi)),
            ("api.deepseek.com", "/chat/completions", Some(Provider::DeepSeek)),
            ("api.cohere.com", "/v2/chat", Some(Provider::Cohere)),
            ("api.cohere.ai", "/v2/chat", Some(Provider::Cohere)),
            ("opencode.ai", "/zen/go/v1/chat/completions", Some(Provider::OpenCode)),
            ("opencode.ai", "/zen/v1/chat/completions", Some(Provider::OpenCodeZen)),
            ("opencode.ai", "/zen/v1/messages", Some(Provider::OpenCodeZen)),
            ("localhost:11434", "/api/chat", Some(Provider::Ollama)),
            ("127.0.0.1", "/api/generate", Some(Provider::Ollama)),
            ("localhost:8080", "/other", None),
            ("example.com", "/v1/chat", None),
        ];
        for (host, path, expected) in cases {
            let got = Provider::from_host_path(host, path);
            assert_eq!(got, *expected, "host = {host:?}, path = {path:?}");
        }
    }

    #[test]
    fn auth_header_per_provider() {
        assert_eq!(Provider::OpenAI.auth_header_name(), "authorization");
        assert_eq!(Provider::Anthropic.auth_header_name(), "x-api-key");
        assert_eq!(Provider::Gemini.auth_header_name(), "x-goog-api-key");
        assert_eq!(Provider::GeminiVertex.auth_header_name(), "authorization");
        assert_eq!(Provider::AzureOpenAI.auth_header_name(), "api-key");
        assert_eq!(Provider::Bedrock.auth_header_name(), "authorization");
        assert_eq!(Provider::Ollama.auth_header_name(), "authorization");
    }

    #[test]
    fn id_keys_are_unique() {
        let provs = [
            Provider::OpenAI,
            Provider::Anthropic,
            Provider::Gemini,
            Provider::GeminiVertex,
            Provider::Bedrock,
            Provider::AzureOpenAI,
            Provider::Mistral,
            Provider::Groq,
            Provider::XAi,
            Provider::DeepSeek,
            Provider::Cohere,
            Provider::Ollama,
            Provider::OpenCode,
            Provider::OpenCodeZen,
            Provider::OpenAICompatible { base_url: Arc::from("https://example.com") },
        ];
        let mut keys: Vec<&str> = provs.iter().map(Provider::id_key).collect();
        let len_before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), len_before, "duplicate id_key");
    }

    #[test]
    fn from_id_key_roundtrip() {
        let known = [
            Provider::OpenAI,
            Provider::Anthropic,
            Provider::Gemini,
            Provider::GeminiVertex,
            Provider::Bedrock,
            Provider::AzureOpenAI,
            Provider::Mistral,
            Provider::Groq,
            Provider::XAi,
            Provider::DeepSeek,
            Provider::Cohere,
            Provider::Ollama,
        ];
        for p in &known {
            assert_eq!(&Provider::from_id_key(p.id_key()), p);
        }
        let compat = Provider::from_id_key("https://custom.example.com");
        assert!(matches!(compat, Provider::OpenAICompatible { .. }));
    }

    #[test]
    fn serde_roundtrip_compatible_variant() {
        let p = Provider::OpenAICompatible { base_url: Arc::from("https://custom.example.com") };
        let j = serde_json::to_string(&p).unwrap();
        let back: Provider = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn canonical_host_uses_base_url_for_compatible() {
        let p = Provider::OpenAICompatible { base_url: Arc::from("https://custom.example.com") };
        assert_eq!(p.canonical_host(), "https://custom.example.com");
    }
}
