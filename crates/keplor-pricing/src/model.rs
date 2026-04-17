//! [`ModelPricing`] — per-model cost rates and capability flags parsed from
//! the LiteLLM pricing catalogue.

use serde::Deserialize;
use smol_str::SmolStr;

/// Per-model pricing and capability metadata.
///
/// All cost fields are stored as **nanodollars per unit** (token, image,
/// second, query).  Raw LiteLLM values (dollars-per-token `f64`) are
/// converted at catalogue-load time via [`dollars_to_nanos`].
///
/// Fields are `Option` where the catalogue entry may omit them — a `None`
/// means the model doesn't support that billing dimension.
#[derive(Debug, Clone)]
pub struct ModelPricing {
    // ── core per-token rates ──────────────────────────────────────────
    /// Standard input cost (nanodollars per token).
    pub input_cost_per_token: i64,
    /// Standard output cost (nanodollars per token).
    pub output_cost_per_token: i64,

    // ── cache rates ───────────────────────────────────────────────────
    /// Cost per cached-read input token (nanodollars).
    pub cache_read_input_token_cost: Option<i64>,
    /// Cost per cache-creation input token (nanodollars).
    pub cache_creation_input_token_cost: Option<i64>,
    /// Cache-creation surcharge for TTLs above 1 hour (Anthropic).
    pub cache_creation_input_token_cost_above_1hr: Option<i64>,
    /// Cache-creation surcharge for contexts above 200k tokens (Anthropic).
    pub cache_creation_input_token_cost_above_200k_tokens: Option<i64>,

    // ── reasoning ─────────────────────────────────────────────────────
    /// Cost per reasoning / thinking token (nanodollars).
    pub output_cost_per_reasoning_token: Option<i64>,

    // ── multimodal ────────────────────────────────────────────────────
    /// Per-image cost (nanodollars).  Not wired into `compute_cost` yet
    /// because [`keplor_core::Usage`] tracks image *tokens*, not image
    /// *count*.  Stored for forward compatibility.
    pub input_cost_per_image: Option<i64>,
    /// Per audio-input-token cost (nanodollars).
    pub input_cost_per_audio_token: Option<i64>,
    /// Per-second video input cost (nanodollars).
    pub input_cost_per_video_per_second: Option<i64>,

    // ── batch / tier ──────────────────────────────────────────────────
    /// Batch-mode input cost (nanodollars per token).
    pub input_cost_per_token_batches: Option<i64>,
    /// Batch-mode output cost (nanodollars per token).
    pub output_cost_per_token_batches: Option<i64>,

    // ── search ────────────────────────────────────────────────────────
    /// Per-search-query cost (nanodollars).
    pub search_context_cost_per_query: Option<i64>,

    // ── above-200k context tier (Anthropic) ───────────────────────────
    /// Input cost per token above 200k context (nanodollars).
    pub input_cost_per_token_above_200k: Option<i64>,
    /// Output cost per token above 200k context (nanodollars).
    pub output_cost_per_token_above_200k: Option<i64>,
    /// Cache-read cost above 200k context (nanodollars).
    pub cache_read_input_token_cost_above_200k: Option<i64>,

    // ── tool-use metadata ─────────────────────────────────────────────
    /// Hidden system-prompt tokens injected for tool use.
    pub tool_use_system_prompt_tokens: Option<u32>,

    // ── limits ────────────────────────────────────────────────────────
    /// Maximum input tokens accepted by the model.
    pub max_input_tokens: Option<u32>,
    /// Maximum output tokens the model can produce.
    pub max_output_tokens: Option<u32>,

    // ── capability flags ──────────────────────────────────────────────
    /// Model supports extended-thinking / reasoning.
    pub supports_reasoning: bool,
    /// Model supports prompt caching (Anthropic / OpenAI).
    pub supports_prompt_caching: bool,
    /// Model accepts image inputs.
    pub supports_vision: bool,

    // ── metadata ──────────────────────────────────────────────────────
    /// ISO-8601 deprecation date (`YYYY-MM-DD`), if any.
    pub deprecation_date: Option<SmolStr>,
    /// User-defined aliases that resolve to this entry.
    pub aliases: Vec<SmolStr>,
    /// Geo multiplier (e.g. non-US Anthropic surcharge).  `None` means 1×.
    pub inference_geo_multiplier: Option<f64>,
    /// LiteLLM provider tag (e.g. `"openai"`, `"anthropic"`).
    pub litellm_provider: SmolStr,
    /// LiteLLM mode tag (e.g. `"chat"`, `"embedding"`).
    pub mode: SmolStr,
}

/// Convert a dollars-per-unit `f64` to nanodollars `i64`.
///
/// # Examples
///
/// ```
/// use keplor_pricing::model::dollars_to_nanos;
/// assert_eq!(dollars_to_nanos(0.000_003), 3_000);  // $3/M input
/// assert_eq!(dollars_to_nanos(0.000_015), 15_000);  // $15/M output
/// assert_eq!(dollars_to_nanos(0.01),  10_000_000);  // $10/1k queries
/// ```
#[must_use]
pub fn dollars_to_nanos(d: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (d * 1e9).round() as i64
    }
}

fn opt_nanos(v: Option<f64>) -> Option<i64> {
    v.map(dollars_to_nanos)
}

// ── Raw serde shape matching LiteLLM JSON ─────────────────────────────

/// Intermediate serde representation of one LiteLLM JSON entry.
///
/// Fields are all `Option` because the catalogue is sparse.
#[derive(Debug, Deserialize)]
pub(crate) struct RawEntry {
    #[serde(default)]
    pub input_cost_per_token: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token: Option<f64>,

    #[serde(default)]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost_above_1hr: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,

    #[serde(default)]
    pub output_cost_per_reasoning_token: Option<f64>,

    #[serde(default)]
    pub input_cost_per_image: Option<f64>,
    #[serde(default)]
    pub input_cost_per_audio_token: Option<f64>,
    #[serde(default)]
    pub input_cost_per_video_per_second: Option<f64>,

    #[serde(default)]
    pub input_cost_per_token_batches: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_batches: Option<f64>,

    #[serde(default)]
    pub search_context_cost_per_query: Option<SearchCostRaw>,

    #[serde(default)]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,

    #[serde(default)]
    pub tool_use_system_prompt_tokens: Option<u32>,

    #[serde(default)]
    pub max_input_tokens: Option<JsonInt>,
    #[serde(default)]
    pub max_output_tokens: Option<JsonInt>,

    #[serde(default)]
    pub supports_reasoning: Option<bool>,
    #[serde(default)]
    pub supports_prompt_caching: Option<bool>,
    #[serde(default)]
    pub supports_vision: Option<bool>,

    #[serde(default)]
    pub deprecation_date: Option<SmolStr>,

    #[serde(default)]
    pub litellm_provider: Option<SmolStr>,
    #[serde(default)]
    pub mode: Option<SmolStr>,
}

/// LiteLLM encodes `search_context_cost_per_query` as either a flat
/// number or a `{low, medium, high}` object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum SearchCostRaw {
    Flat(f64),
    Tiered {
        #[serde(default)]
        search_context_size_medium: Option<f64>,
        #[serde(default)]
        search_context_size_high: Option<f64>,
        #[serde(default)]
        search_context_size_low: Option<f64>,
    },
}

impl SearchCostRaw {
    fn to_nanos(&self) -> i64 {
        match self {
            Self::Flat(v) => dollars_to_nanos(*v),
            Self::Tiered {
                search_context_size_medium,
                search_context_size_high,
                search_context_size_low,
            } => {
                let v = search_context_size_medium
                    .or(*search_context_size_high)
                    .or(*search_context_size_low)
                    .unwrap_or(0.0);
                dollars_to_nanos(v)
            },
        }
    }
}

/// Some LiteLLM entries encode `max_input_tokens` / `max_output_tokens`
/// as a string ("LEGACY") rather than an integer.  Accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum JsonInt {
    Num(u32),
    #[allow(dead_code)]
    Str(String),
}

impl JsonInt {
    fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Num(n) => Some(*n),
            Self::Str(_) => None,
        }
    }
}

impl RawEntry {
    /// Convert to the internal [`ModelPricing`], filling in zero-costs
    /// for required fields when the source omits them.
    pub(crate) fn into_pricing(self) -> ModelPricing {
        ModelPricing {
            input_cost_per_token: dollars_to_nanos(self.input_cost_per_token.unwrap_or(0.0)),
            output_cost_per_token: dollars_to_nanos(self.output_cost_per_token.unwrap_or(0.0)),

            cache_read_input_token_cost: opt_nanos(self.cache_read_input_token_cost),
            cache_creation_input_token_cost: opt_nanos(self.cache_creation_input_token_cost),
            cache_creation_input_token_cost_above_1hr: opt_nanos(
                self.cache_creation_input_token_cost_above_1hr,
            ),
            cache_creation_input_token_cost_above_200k_tokens: opt_nanos(
                self.cache_creation_input_token_cost_above_200k_tokens,
            ),

            output_cost_per_reasoning_token: opt_nanos(self.output_cost_per_reasoning_token),

            input_cost_per_image: opt_nanos(self.input_cost_per_image),
            input_cost_per_audio_token: opt_nanos(self.input_cost_per_audio_token),
            input_cost_per_video_per_second: opt_nanos(self.input_cost_per_video_per_second),

            input_cost_per_token_batches: opt_nanos(self.input_cost_per_token_batches),
            output_cost_per_token_batches: opt_nanos(self.output_cost_per_token_batches),

            search_context_cost_per_query: self
                .search_context_cost_per_query
                .as_ref()
                .map(SearchCostRaw::to_nanos),

            input_cost_per_token_above_200k: opt_nanos(self.input_cost_per_token_above_200k_tokens),
            output_cost_per_token_above_200k: opt_nanos(
                self.output_cost_per_token_above_200k_tokens,
            ),
            cache_read_input_token_cost_above_200k: opt_nanos(
                self.cache_read_input_token_cost_above_200k_tokens,
            ),

            tool_use_system_prompt_tokens: self.tool_use_system_prompt_tokens,

            max_input_tokens: self.max_input_tokens.as_ref().and_then(JsonInt::as_u32),
            max_output_tokens: self.max_output_tokens.as_ref().and_then(JsonInt::as_u32),

            supports_reasoning: self.supports_reasoning.unwrap_or(false),
            supports_prompt_caching: self.supports_prompt_caching.unwrap_or(false),
            supports_vision: self.supports_vision.unwrap_or(false),

            deprecation_date: self.deprecation_date,
            aliases: Vec::new(),
            inference_geo_multiplier: None,
            litellm_provider: self.litellm_provider.unwrap_or_else(|| SmolStr::new("unknown")),
            mode: self.mode.unwrap_or_else(|| SmolStr::new("chat")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollars_to_nanos_exact_common_rates() {
        assert_eq!(dollars_to_nanos(0.000_003), 3_000);
        assert_eq!(dollars_to_nanos(0.000_015), 15_000);
        assert_eq!(dollars_to_nanos(0.01), 10_000_000);
        assert_eq!(dollars_to_nanos(0.0), 0);
        assert_eq!(dollars_to_nanos(1.0), 1_000_000_000);
    }

    #[test]
    fn dollars_to_nanos_half_cent_rates() {
        // $2.50/M tokens = $0.0000025/token
        assert_eq!(dollars_to_nanos(0.000_002_5), 2_500);
    }

    #[test]
    fn raw_entry_parse_minimal() {
        let json = r#"{"input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015}"#;
        let raw: RawEntry = serde_json::from_str(json).unwrap();
        let p = raw.into_pricing();
        assert_eq!(p.input_cost_per_token, 3_000);
        assert_eq!(p.output_cost_per_token, 15_000);
        assert!(p.cache_read_input_token_cost.is_none());
    }

    #[test]
    fn raw_entry_parse_with_caching() {
        let json = r#"{
            "input_cost_per_token": 0.000003,
            "output_cost_per_token": 0.000015,
            "cache_read_input_token_cost": 0.0000003,
            "cache_creation_input_token_cost": 0.00000375,
            "cache_creation_input_token_cost_above_1hr": 0.000006,
            "litellm_provider": "anthropic",
            "supports_prompt_caching": true
        }"#;
        let raw: RawEntry = serde_json::from_str(json).unwrap();
        let p = raw.into_pricing();
        assert_eq!(p.cache_read_input_token_cost, Some(300));
        assert_eq!(p.cache_creation_input_token_cost, Some(3_750));
        assert_eq!(p.cache_creation_input_token_cost_above_1hr, Some(6_000));
        assert!(p.supports_prompt_caching);
        assert_eq!(p.litellm_provider, "anthropic");
    }

    #[test]
    fn search_cost_flat_and_tiered() {
        let flat: SearchCostRaw = serde_json::from_str("0.01").unwrap();
        assert_eq!(flat.to_nanos(), 10_000_000);

        let tiered: SearchCostRaw = serde_json::from_str(
            r#"{"search_context_size_low": 0.005, "search_context_size_medium": 0.01, "search_context_size_high": 0.025}"#,
        )
        .unwrap();
        assert_eq!(tiered.to_nanos(), 10_000_000);
    }

    #[test]
    fn json_int_string_is_none() {
        let ji: JsonInt = serde_json::from_str(r#""LEGACY""#).unwrap();
        assert!(ji.as_u32().is_none());

        let ji: JsonInt = serde_json::from_str("128000").unwrap();
        assert_eq!(ji.as_u32(), Some(128_000));
    }
}
