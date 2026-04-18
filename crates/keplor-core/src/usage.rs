//! [`Usage`] — every token dimension Keplor accounts for, plus the merge
//! helper for streaming-delta accumulation and the provider-aware
//! "billable input" calculator.

use serde::{Deserialize, Serialize};

use crate::Provider;

/// Per-request token / media counters.
///
/// All fields are zero by default.  Streaming deltas are combined via
/// [`Usage::merge`] — note that *provider adapters* are responsible for
/// converting cumulative-total streams (Anthropic, Gemini) into
/// per-chunk deltas before calling `merge`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Prompt-side tokens.  On OpenAI-family APIs this is already
    /// inclusive of cached tokens; on Anthropic it excludes them (see
    /// [`Usage::total_billable_input_tokens`]).
    pub input_tokens: u32,

    /// Completion-side tokens (response body only; does not include
    /// reasoning tokens for OpenAI/Gemini, which are counted separately).
    pub output_tokens: u32,

    /// Tokens read from a cache hit (Anthropic / OpenAI prompt caching).
    pub cache_read_input_tokens: u32,

    /// Tokens written into a cache on this turn (Anthropic cache-write,
    /// counted at 1.25× the input rate on the wire but reported as raw
    /// tokens here).
    pub cache_creation_input_tokens: u32,

    /// Reasoning / thinking tokens — OpenAI Responses `reasoning_tokens`,
    /// Gemini Vertex `thoughtsTokenCount`, Bedrock `reasoningContent`.
    pub reasoning_tokens: u32,

    /// Audio input tokens (OpenAI multimodal, Gemini audio modality).
    pub audio_input_tokens: u32,

    /// Audio output tokens.
    pub audio_output_tokens: u32,

    /// Image input tokens (per the provider's image tokeniser, if any).
    pub image_tokens: u32,

    /// Video input, measured in seconds (Gemini video modality).
    pub video_seconds: u32,

    /// Tokens spent on provider-side tool-use orchestration (Gemini
    /// `toolUsePromptTokenCount`, Anthropic tool-result tokens).
    pub tool_use_tokens: u32,

    /// Number of search queries billed separately (some Cohere / Gemini
    /// flows).
    pub search_queries: u32,
}

impl Usage {
    /// Accumulate the counters in `delta` into `self`, saturating every
    /// field at [`u32::MAX`].
    ///
    /// Use this to combine per-chunk deltas during stream reassembly.
    pub fn merge(&mut self, delta: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(delta.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(delta.output_tokens);
        self.cache_read_input_tokens =
            self.cache_read_input_tokens.saturating_add(delta.cache_read_input_tokens);
        self.cache_creation_input_tokens =
            self.cache_creation_input_tokens.saturating_add(delta.cache_creation_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(delta.reasoning_tokens);
        self.audio_input_tokens = self.audio_input_tokens.saturating_add(delta.audio_input_tokens);
        self.audio_output_tokens =
            self.audio_output_tokens.saturating_add(delta.audio_output_tokens);
        self.image_tokens = self.image_tokens.saturating_add(delta.image_tokens);
        self.video_seconds = self.video_seconds.saturating_add(delta.video_seconds);
        self.tool_use_tokens = self.tool_use_tokens.saturating_add(delta.tool_use_tokens);
        self.search_queries = self.search_queries.saturating_add(delta.search_queries);
    }

    /// Total input tokens that should be billed as *prompt* tokens for
    /// `provider`.
    ///
    /// Every provider's notion of "input" differs:
    ///
    /// | Provider          | Formula                                                   |
    /// |-------------------|-----------------------------------------------------------|
    /// | OpenAI + Azure    | `input_tokens` (already inclusive of cache reads)         |
    /// | Anthropic         | `input_tokens + cache_creation + cache_read`              |
    /// | Bedrock (Claude)  | `input_tokens + cache_creation + cache_read`              |
    /// | Gemini (AI Studio)| `input_tokens`                                            |
    /// | Gemini Vertex     | `input_tokens` (candidates/thoughts/tools are *output*)   |
    /// | Mistral, Groq, xAI, DeepSeek, Cohere, Ollama, OpenAI-compat | `input_tokens` |
    #[must_use]
    pub fn total_billable_input_tokens(&self, provider: &Provider) -> u32 {
        match provider {
            Provider::Anthropic | Provider::Bedrock => self
                .input_tokens
                .saturating_add(self.cache_creation_input_tokens)
                .saturating_add(self.cache_read_input_tokens),
            Provider::OpenAI
            | Provider::AzureOpenAI
            | Provider::Gemini
            | Provider::GeminiVertex
            | Provider::Mistral
            | Provider::Groq
            | Provider::XAi
            | Provider::DeepSeek
            | Provider::Cohere
            | Provider::OpenRouter
            | Provider::Ollama
            | Provider::OpenAICompatible { .. } => self.input_tokens,
        }
    }

    /// Total output-side tokens (completion + reasoning).  Useful for
    /// single-number "response size" metrics.
    #[must_use]
    pub fn total_output_tokens(&self) -> u32 {
        self.output_tokens.saturating_add(self.reasoning_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_is_saturating_add() {
        let mut a = Usage { input_tokens: 100, output_tokens: 50, ..Usage::default() };
        let b = Usage {
            input_tokens: 20,
            output_tokens: 10,
            cache_read_input_tokens: 7,
            reasoning_tokens: 3,
            ..Usage::default()
        };
        a.merge(&b);
        assert_eq!(a.input_tokens, 120);
        assert_eq!(a.output_tokens, 60);
        assert_eq!(a.cache_read_input_tokens, 7);
        assert_eq!(a.reasoning_tokens, 3);
    }

    #[test]
    fn merge_saturates_at_u32_max() {
        let mut a = Usage { input_tokens: u32::MAX - 1, ..Usage::default() };
        let b = Usage { input_tokens: 10, ..Usage::default() };
        a.merge(&b);
        assert_eq!(a.input_tokens, u32::MAX);
    }

    #[test]
    fn merge_accumulates_delta_stream() {
        // Simulate a 3-chunk stream where each chunk reports its own delta.
        let deltas = [
            Usage { input_tokens: 12, output_tokens: 1, ..Usage::default() },
            Usage { output_tokens: 3, ..Usage::default() },
            Usage { output_tokens: 5, reasoning_tokens: 2, ..Usage::default() },
        ];
        let mut acc = Usage::default();
        for d in &deltas {
            acc.merge(d);
        }
        assert_eq!(acc.input_tokens, 12);
        assert_eq!(acc.output_tokens, 9);
        assert_eq!(acc.reasoning_tokens, 2);
    }

    #[test]
    fn billable_input_openai_includes_cache() {
        // OpenAI's `prompt_tokens` already counts cached tokens, so
        // `input_tokens` alone is correct — don't double-add cache_read.
        let u = Usage {
            input_tokens: 1000,
            cache_read_input_tokens: 400,
            cache_creation_input_tokens: 0,
            ..Usage::default()
        };
        assert_eq!(u.total_billable_input_tokens(&Provider::OpenAI), 1000);
        assert_eq!(u.total_billable_input_tokens(&Provider::AzureOpenAI), 1000);
    }

    #[test]
    fn billable_input_anthropic_sums_cache_fields() {
        let u = Usage {
            input_tokens: 100,
            cache_read_input_tokens: 300,
            cache_creation_input_tokens: 50,
            ..Usage::default()
        };
        assert_eq!(u.total_billable_input_tokens(&Provider::Anthropic), 450);
        assert_eq!(u.total_billable_input_tokens(&Provider::Bedrock), 450);
    }

    #[test]
    fn billable_input_gemini_excludes_thoughts() {
        // Gemini Vertex reports thoughtsTokenCount as part of totalTokenCount
        // but NOT as part of promptTokenCount — so input_tokens already
        // excludes thoughts and is the correct billable input.
        let u = Usage { input_tokens: 2000, reasoning_tokens: 500, ..Usage::default() };
        assert_eq!(u.total_billable_input_tokens(&Provider::Gemini), 2000);
        assert_eq!(u.total_billable_input_tokens(&Provider::GeminiVertex), 2000);
    }

    #[test]
    fn billable_input_ollama_and_others() {
        let u = Usage { input_tokens: 42, ..Usage::default() };
        assert_eq!(u.total_billable_input_tokens(&Provider::Ollama), 42);
        assert_eq!(u.total_billable_input_tokens(&Provider::Mistral), 42);
        assert_eq!(u.total_billable_input_tokens(&Provider::Groq), 42);
        assert_eq!(u.total_billable_input_tokens(&Provider::XAi), 42);
        assert_eq!(u.total_billable_input_tokens(&Provider::DeepSeek), 42);
        assert_eq!(u.total_billable_input_tokens(&Provider::Cohere), 42);
    }

    #[test]
    fn total_output_sums_output_and_reasoning() {
        let u = Usage { output_tokens: 100, reasoning_tokens: 50, ..Usage::default() };
        assert_eq!(u.total_output_tokens(), 150);
    }

    #[test]
    fn default_is_all_zero() {
        let u = Usage::default();
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.search_queries, 0);
    }

    #[test]
    fn serde_defaults_missing_fields_to_zero() {
        let j = r#"{"input_tokens": 12, "output_tokens": 34}"#;
        let u: Usage = serde_json::from_str(j).unwrap();
        assert_eq!(u.input_tokens, 12);
        assert_eq!(u.output_tokens, 34);
        assert_eq!(u.reasoning_tokens, 0);
        assert_eq!(u.search_queries, 0);
    }
}
