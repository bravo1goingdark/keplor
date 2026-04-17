//! Cost computation engine: given `(ModelPricing, Usage, CostOpts)`, return
//! a [`Cost`] in nanodollars.

use keplor_core::{Cost, Provider, Usage};

use crate::model::ModelPricing;

/// Options that affect cost calculation beyond the static model pricing.
#[derive(Debug, Clone, Default)]
pub struct CostOpts {
    /// Whether this request was submitted via the batch API.
    pub is_batch: bool,
    /// Service tier (OpenAI priority/standard/flex).
    pub service_tier: ServiceTier,
    /// Geographic region for inference (affects Anthropic pricing).
    pub inference_geo: InferenceGeo,
    /// Prompt-cache TTL bucket (Anthropic 5 min vs. 1 hr cache).
    pub cache_ttl: CacheTtl,
    /// Context-length bucket (Anthropic above/below 200k surcharge).
    pub context_bucket: ContextBucket,
}

/// OpenAI service tier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServiceTier {
    /// Standard tier (default).
    #[default]
    Standard,
    /// Flex tier (cheapest, lowest priority).
    Flex,
    /// Priority tier (higher latency SLA, premium rate).
    Priority,
}

/// Inference geography.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InferenceGeo {
    /// US region (no surcharge).
    #[default]
    Us,
    /// Non-US region (surcharge may apply on some providers).
    NonUs,
}

/// Prompt-cache TTL bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheTtl {
    /// Default 5-minute TTL.
    #[default]
    Minutes5,
    /// Extended 1-hour TTL (Anthropic surcharge).
    Hours1,
}

/// Context-length bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContextBucket {
    /// Context ≤ 200k tokens (standard rate).
    #[default]
    Standard,
    /// Context > 200k tokens (Anthropic surcharge).
    Above200k,
}

/// Compute the total cost of a request.
///
/// The function correctly distinguishes **Anthropic-family** providers
/// (where `input_tokens` *excludes* cached tokens) from **OpenAI-family**
/// providers (where `input_tokens` *includes* cached-read tokens).
///
/// # Examples
///
/// ```
/// use keplor_core::{Cost, Provider, Usage};
/// use keplor_pricing::compute::{compute_cost, CostOpts};
/// use keplor_pricing::model::ModelPricing;
///
/// // GPT-4o: $2.50/M in, $10/M out
/// let pricing = ModelPricing {
///     input_cost_per_token: 2_500,   // nanodollars
///     output_cost_per_token: 10_000,
///     cache_read_input_token_cost: Some(1_250),
///     ..ModelPricing::zeroed()
/// };
/// let usage = Usage {
///     input_tokens: 1000,
///     output_tokens: 500,
///     cache_read_input_tokens: 200,
///     ..Usage::default()
/// };
/// let cost = compute_cost(&Provider::OpenAI, &pricing, &usage, &CostOpts::default());
/// // (1000 - 200) * 2500 + 200 * 1250 + 500 * 10000
/// // = 2_000_000 + 250_000 + 5_000_000 = 7_250_000
/// assert_eq!(cost, Cost::from_nanodollars(7_250_000));
/// ```
///
/// ```
/// use keplor_core::{Cost, Provider, Usage};
/// use keplor_pricing::compute::{compute_cost, CostOpts};
/// use keplor_pricing::model::ModelPricing;
///
/// // Anthropic Claude: input_tokens EXCLUDES cache fields
/// let pricing = ModelPricing {
///     input_cost_per_token: 3_000,
///     output_cost_per_token: 15_000,
///     cache_read_input_token_cost: Some(300),
///     cache_creation_input_token_cost: Some(3_750),
///     ..ModelPricing::zeroed()
/// };
/// let usage = Usage {
///     input_tokens: 100,
///     output_tokens: 200,
///     cache_read_input_tokens: 500,
///     cache_creation_input_tokens: 50,
///     ..Usage::default()
/// };
/// let cost = compute_cost(&Provider::Anthropic, &pricing, &usage, &CostOpts::default());
/// // 100 * 3000 + 500 * 300 + 50 * 3750 + 200 * 15000
/// // = 300_000 + 150_000 + 187_500 + 3_000_000 = 3_637_500
/// assert_eq!(cost, Cost::from_nanodollars(3_637_500));
/// ```
///
/// Anthropic cache-write with 1-hour TTL surcharge:
///
/// ```
/// use keplor_core::{Cost, Provider, Usage};
/// use keplor_pricing::compute::{compute_cost, CacheTtl, CostOpts};
/// use keplor_pricing::model::ModelPricing;
///
/// let pricing = ModelPricing {
///     input_cost_per_token: 3_000,
///     output_cost_per_token: 15_000,
///     cache_creation_input_token_cost: Some(3_750),
///     cache_creation_input_token_cost_above_1hr: Some(6_000),
///     ..ModelPricing::zeroed()
/// };
/// let usage = Usage {
///     input_tokens: 100,
///     cache_creation_input_tokens: 50,
///     ..Usage::default()
/// };
/// let opts = CostOpts { cache_ttl: CacheTtl::Hours1, ..CostOpts::default() };
/// let cost = compute_cost(&Provider::Anthropic, &pricing, &usage, &opts);
/// // 100 * 3000 + 50 * 6000 (1hr surcharge) = 600_000
/// assert_eq!(cost, Cost::from_nanodollars(600_000));
/// ```
///
/// Bedrock uses the same Anthropic cache-write billing path:
///
/// ```
/// use keplor_core::{Cost, Provider, Usage};
/// use keplor_pricing::compute::{compute_cost, CostOpts};
/// use keplor_pricing::model::ModelPricing;
///
/// let pricing = ModelPricing {
///     input_cost_per_token: 3_000,
///     output_cost_per_token: 15_000,
///     cache_read_input_token_cost: Some(300),
///     cache_creation_input_token_cost: Some(3_750),
///     ..ModelPricing::zeroed()
/// };
/// let usage = Usage {
///     input_tokens: 1000,
///     output_tokens: 500,
///     cache_read_input_tokens: 200,
///     cache_creation_input_tokens: 100,
///     ..Usage::default()
/// };
/// let cost_bedrock = compute_cost(&Provider::Bedrock, &pricing, &usage, &CostOpts::default());
/// let cost_anthropic = compute_cost(&Provider::Anthropic, &pricing, &usage, &CostOpts::default());
/// assert_eq!(cost_bedrock, cost_anthropic);
/// ```
///
/// Gemini thoughts-included-in-candidates quirk: reasoning tokens are
/// billed separately at the output rate (fallback when no dedicated
/// reasoning rate exists):
///
/// ```
/// use keplor_core::{Cost, Provider, Usage};
/// use keplor_pricing::compute::{compute_cost, CostOpts};
/// use keplor_pricing::model::ModelPricing;
///
/// let pricing = ModelPricing {
///     input_cost_per_token: 100,
///     output_cost_per_token: 400,
///     ..ModelPricing::zeroed()  // no dedicated reasoning rate
/// };
/// let usage = Usage {
///     input_tokens: 1000,
///     output_tokens: 200,
///     reasoning_tokens: 300,
///     ..Usage::default()
/// };
/// let cost = compute_cost(&Provider::Gemini, &pricing, &usage, &CostOpts::default());
/// // 1000*100 + 200*400 + 300*400 = 100_000 + 80_000 + 120_000
/// assert_eq!(cost, Cost::from_nanodollars(300_000));
/// ```
#[must_use]
pub fn compute_cost(
    provider: &Provider,
    pricing: &ModelPricing,
    usage: &Usage,
    opts: &CostOpts,
) -> Cost {
    let mut total: i64 = 0;

    // ── pick base rates (batch / standard) ────────────────────────
    let input_rate = pick_input_rate(pricing, opts);
    let output_rate = pick_output_rate(pricing, opts);

    // ── input billing ─────────────────────────────────────────────
    match provider {
        Provider::Anthropic | Provider::Bedrock => {
            total = total.saturating_add(mul_tokens(usage.input_tokens, input_rate));

            let cache_read_rate = pick_cache_read_rate(pricing, opts);
            total =
                total.saturating_add(mul_tokens(usage.cache_read_input_tokens, cache_read_rate));

            let cache_create_rate = pick_cache_create_rate(pricing, opts);
            total = total
                .saturating_add(mul_tokens(usage.cache_creation_input_tokens, cache_create_rate));
        },
        _ => {
            // OpenAI-family: input_tokens already includes cache reads.
            let uncached = usage.input_tokens.saturating_sub(usage.cache_read_input_tokens);
            total = total.saturating_add(mul_tokens(uncached, input_rate));

            let cache_read_rate = pricing.cache_read_input_token_cost.unwrap_or(input_rate);
            total =
                total.saturating_add(mul_tokens(usage.cache_read_input_tokens, cache_read_rate));

            if let Some(rate) = pricing.cache_creation_input_token_cost {
                total = total.saturating_add(mul_tokens(usage.cache_creation_input_tokens, rate));
            }
        },
    }

    // ── output billing ────────────────────────────────────────────
    total = total.saturating_add(mul_tokens(usage.output_tokens, output_rate));

    // ── reasoning tokens ──────────────────────────────────────────
    let reasoning_rate = pricing.output_cost_per_reasoning_token.unwrap_or(output_rate);
    total = total.saturating_add(mul_tokens(usage.reasoning_tokens, reasoning_rate));

    // ── audio tokens ──────────────────────────────────────────────
    if let Some(rate) = pricing.input_cost_per_audio_token {
        total = total.saturating_add(mul_tokens(usage.audio_input_tokens, rate));
    }

    // ── video seconds ─────────────────────────────────────────────
    if let Some(rate) = pricing.input_cost_per_video_per_second {
        total = total.saturating_add(mul_tokens(usage.video_seconds, rate));
    }

    // ── search queries ────────────────────────────────────────────
    if let Some(rate) = pricing.search_context_cost_per_query {
        total = total.saturating_add(mul_tokens(usage.search_queries, rate));
    }

    // ── geo multiplier (Anthropic non-US) ─────────────────────────
    if let Some(mult) = pricing.inference_geo_multiplier {
        if opts.inference_geo == InferenceGeo::NonUs {
            #[allow(clippy::cast_possible_truncation)]
            {
                total = (total as f64 * mult).round() as i64;
            }
        }
    }

    Cost::from_nanodollars(total)
}

/// Pick input rate considering batch + above-200k tiers.
fn pick_input_rate(p: &ModelPricing, opts: &CostOpts) -> i64 {
    if opts.is_batch {
        if let Some(r) = p.input_cost_per_token_batches {
            return r;
        }
    }
    if opts.context_bucket == ContextBucket::Above200k {
        if let Some(r) = p.input_cost_per_token_above_200k {
            return r;
        }
    }
    p.input_cost_per_token
}

/// Pick output rate considering batch + above-200k tiers.
fn pick_output_rate(p: &ModelPricing, opts: &CostOpts) -> i64 {
    if opts.is_batch {
        if let Some(r) = p.output_cost_per_token_batches {
            return r;
        }
    }
    if opts.context_bucket == ContextBucket::Above200k {
        if let Some(r) = p.output_cost_per_token_above_200k {
            return r;
        }
    }
    p.output_cost_per_token
}

/// Pick cache-read rate (Anthropic above-200k variant if applicable).
fn pick_cache_read_rate(p: &ModelPricing, opts: &CostOpts) -> i64 {
    if opts.context_bucket == ContextBucket::Above200k {
        if let Some(r) = p.cache_read_input_token_cost_above_200k {
            return r;
        }
    }
    p.cache_read_input_token_cost.unwrap_or(p.input_cost_per_token)
}

/// Pick cache-creation rate (above-1hr and above-200k tiers).
fn pick_cache_create_rate(p: &ModelPricing, opts: &CostOpts) -> i64 {
    if opts.context_bucket == ContextBucket::Above200k {
        if let Some(r) = p.cache_creation_input_token_cost_above_200k_tokens {
            return r;
        }
    }
    if opts.cache_ttl == CacheTtl::Hours1 {
        if let Some(r) = p.cache_creation_input_token_cost_above_1hr {
            return r;
        }
    }
    p.cache_creation_input_token_cost.unwrap_or(p.input_cost_per_token)
}

/// Multiply token count by nanodollar rate, saturating.
fn mul_tokens(tokens: u32, nanos_per_token: i64) -> i64 {
    i64::from(tokens).saturating_mul(nanos_per_token)
}

impl ModelPricing {
    /// A pricing entry with all costs zeroed.  Useful as a `..` base in
    /// tests and doc-examples.
    ///
    /// # Examples
    ///
    /// ```
    /// use keplor_pricing::model::ModelPricing;
    /// let p = ModelPricing {
    ///     input_cost_per_token: 3_000,
    ///     output_cost_per_token: 15_000,
    ///     ..ModelPricing::zeroed()
    /// };
    /// assert_eq!(p.cache_read_input_token_cost, None);
    /// ```
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            input_cost_per_token: 0,
            output_cost_per_token: 0,
            cache_read_input_token_cost: None,
            cache_creation_input_token_cost: None,
            cache_creation_input_token_cost_above_1hr: None,
            cache_creation_input_token_cost_above_200k_tokens: None,
            output_cost_per_reasoning_token: None,
            input_cost_per_image: None,
            input_cost_per_audio_token: None,
            input_cost_per_video_per_second: None,
            input_cost_per_token_batches: None,
            output_cost_per_token_batches: None,
            search_context_cost_per_query: None,
            input_cost_per_token_above_200k: None,
            output_cost_per_token_above_200k: None,
            cache_read_input_token_cost_above_200k: None,
            tool_use_system_prompt_tokens: None,
            max_input_tokens: None,
            max_output_tokens: None,
            supports_reasoning: false,
            supports_prompt_caching: false,
            supports_vision: false,
            deprecation_date: None,
            aliases: Vec::new(),
            inference_geo_multiplier: None,
            litellm_provider: smol_str::SmolStr::new("unknown"),
            mode: smol_str::SmolStr::new("chat"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpt4o_pricing() -> ModelPricing {
        ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            cache_read_input_token_cost: Some(1_250),
            input_cost_per_token_batches: Some(1_250),
            output_cost_per_token_batches: Some(5_000),
            ..ModelPricing::zeroed()
        }
    }

    fn claude_sonnet_pricing() -> ModelPricing {
        ModelPricing {
            input_cost_per_token: 3_000,
            output_cost_per_token: 15_000,
            cache_read_input_token_cost: Some(300),
            cache_creation_input_token_cost: Some(3_750),
            cache_creation_input_token_cost_above_1hr: Some(6_000),
            cache_creation_input_token_cost_above_200k_tokens: Some(7_500),
            input_cost_per_token_above_200k: Some(6_000),
            output_cost_per_token_above_200k: Some(22_500),
            cache_read_input_token_cost_above_200k: Some(600),
            ..ModelPricing::zeroed()
        }
    }

    #[test]
    fn openai_standard_no_cache() {
        let p = gpt4o_pricing();
        let u = Usage { input_tokens: 1000, output_tokens: 500, ..Usage::default() };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // 1000 * 2500 + 500 * 10000 = 7_500_000
        assert_eq!(c, Cost::from_nanodollars(7_500_000));
    }

    #[test]
    fn openai_with_cache_read() {
        let p = gpt4o_pricing();
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 200,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // (1000 - 200) * 2500 + 200 * 1250 + 500 * 10000
        // = 2_000_000 + 250_000 + 5_000_000 = 7_250_000
        assert_eq!(c, Cost::from_nanodollars(7_250_000));
    }

    #[test]
    fn openai_batch_mode() {
        let p = gpt4o_pricing();
        let u = Usage { input_tokens: 1000, output_tokens: 500, ..Usage::default() };
        let opts = CostOpts { is_batch: true, ..CostOpts::default() };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &opts);
        // 1000 * 1250 + 500 * 5000 = 3_750_000
        assert_eq!(c, Cost::from_nanodollars(3_750_000));
    }

    #[test]
    fn anthropic_standard_no_cache() {
        let p = claude_sonnet_pricing();
        let u = Usage { input_tokens: 1000, output_tokens: 500, ..Usage::default() };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &CostOpts::default());
        // 1000 * 3000 + 500 * 15000 = 10_500_000
        assert_eq!(c, Cost::from_nanodollars(10_500_000));
    }

    #[test]
    fn anthropic_with_cache_read_and_write() {
        let p = claude_sonnet_pricing();
        let u = Usage {
            input_tokens: 100,
            output_tokens: 200,
            cache_read_input_tokens: 500,
            cache_creation_input_tokens: 50,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &CostOpts::default());
        // 100 * 3000 + 500 * 300 + 50 * 3750 + 200 * 15000
        // = 300_000 + 150_000 + 187_500 + 3_000_000 = 3_637_500
        assert_eq!(c, Cost::from_nanodollars(3_637_500));
    }

    #[test]
    fn anthropic_cache_write_above_1hr() {
        let p = claude_sonnet_pricing();
        let u = Usage { input_tokens: 100, cache_creation_input_tokens: 50, ..Usage::default() };
        let opts = CostOpts { cache_ttl: CacheTtl::Hours1, ..CostOpts::default() };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &opts);
        // 100 * 3000 + 50 * 6000 = 300_000 + 300_000 = 600_000
        assert_eq!(c, Cost::from_nanodollars(600_000));
    }

    #[test]
    fn anthropic_above_200k_context() {
        let p = claude_sonnet_pricing();
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 200,
            cache_creation_input_tokens: 100,
            ..Usage::default()
        };
        let opts = CostOpts { context_bucket: ContextBucket::Above200k, ..CostOpts::default() };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &opts);
        // input: 1000 * 6000 = 6_000_000
        // cache_read: 200 * 600 = 120_000
        // cache_create: 100 * 7500 = 750_000
        // output: 500 * 22_500 = 11_250_000
        // total = 18_120_000
        assert_eq!(c, Cost::from_nanodollars(18_120_000));
    }

    #[test]
    fn bedrock_uses_anthropic_billing() {
        let p = claude_sonnet_pricing();
        let u = Usage {
            input_tokens: 100,
            cache_read_input_tokens: 500,
            cache_creation_input_tokens: 50,
            output_tokens: 200,
            ..Usage::default()
        };
        let c_anth = compute_cost(&Provider::Anthropic, &p, &u, &CostOpts::default());
        let c_bed = compute_cost(&Provider::Bedrock, &p, &u, &CostOpts::default());
        assert_eq!(c_anth, c_bed);
    }

    #[test]
    fn reasoning_tokens_billed_at_reasoning_rate() {
        let p = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            output_cost_per_reasoning_token: Some(15_000),
            ..ModelPricing::zeroed()
        };
        let u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            reasoning_tokens: 200,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // 100 * 2500 + 50 * 10_000 + 200 * 15_000
        // = 250_000 + 500_000 + 3_000_000 = 3_750_000
        assert_eq!(c, Cost::from_nanodollars(3_750_000));
    }

    #[test]
    fn reasoning_tokens_fallback_to_output_rate() {
        let p = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            ..ModelPricing::zeroed()
        };
        let u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            reasoning_tokens: 200,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // 100 * 2500 + 50 * 10_000 + 200 * 10_000
        // = 250_000 + 500_000 + 2_000_000 = 2_750_000
        assert_eq!(c, Cost::from_nanodollars(2_750_000));
    }

    #[test]
    fn audio_tokens_billed() {
        let p = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            input_cost_per_audio_token: Some(100_000),
            ..ModelPricing::zeroed()
        };
        let u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            audio_input_tokens: 30,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // 100 * 2500 + 50 * 10_000 + 30 * 100_000
        // = 250_000 + 500_000 + 3_000_000 = 3_750_000
        assert_eq!(c, Cost::from_nanodollars(3_750_000));
    }

    #[test]
    fn video_seconds_billed() {
        let p = ModelPricing {
            input_cost_per_token: 100,
            output_cost_per_token: 400,
            input_cost_per_video_per_second: Some(1_000_000),
            ..ModelPricing::zeroed()
        };
        let u =
            Usage { input_tokens: 1000, output_tokens: 100, video_seconds: 10, ..Usage::default() };
        let c = compute_cost(&Provider::Gemini, &p, &u, &CostOpts::default());
        // 1000 * 100 + 100 * 400 + 10 * 1_000_000
        // = 100_000 + 40_000 + 10_000_000 = 10_140_000
        assert_eq!(c, Cost::from_nanodollars(10_140_000));
    }

    #[test]
    fn search_queries_billed() {
        let p = ModelPricing {
            input_cost_per_token: 3_000,
            output_cost_per_token: 15_000,
            search_context_cost_per_query: Some(10_000_000),
            ..ModelPricing::zeroed()
        };
        let u =
            Usage { input_tokens: 100, output_tokens: 50, search_queries: 2, ..Usage::default() };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &CostOpts::default());
        // 100 * 3000 + 50 * 15000 + 2 * 10_000_000
        // = 300_000 + 750_000 + 20_000_000 = 21_050_000
        assert_eq!(c, Cost::from_nanodollars(21_050_000));
    }

    #[test]
    fn geo_multiplier_applied() {
        let p = ModelPricing {
            input_cost_per_token: 3_000,
            output_cost_per_token: 15_000,
            inference_geo_multiplier: Some(1.25),
            ..ModelPricing::zeroed()
        };
        let u = Usage { input_tokens: 1000, output_tokens: 500, ..Usage::default() };
        let opts = CostOpts { inference_geo: InferenceGeo::NonUs, ..CostOpts::default() };
        let c = compute_cost(&Provider::Anthropic, &p, &u, &opts);
        let base = 1000 * 3000 + 500 * 15000; // 10_500_000
        let expected = (base as f64 * 1.25).round() as i64; // 13_125_000
        assert_eq!(c, Cost::from_nanodollars(expected));
    }

    #[test]
    fn geo_multiplier_not_applied_for_us() {
        let p = ModelPricing {
            input_cost_per_token: 3_000,
            output_cost_per_token: 15_000,
            inference_geo_multiplier: Some(1.25),
            ..ModelPricing::zeroed()
        };
        let u = Usage { input_tokens: 1000, output_tokens: 500, ..Usage::default() };
        let c = compute_cost(
            &Provider::Anthropic,
            &p,
            &u,
            &CostOpts::default(), // US
        );
        assert_eq!(c, Cost::from_nanodollars(10_500_000));
    }

    #[test]
    fn zero_usage_is_zero_cost() {
        let p = gpt4o_pricing();
        let c = compute_cost(&Provider::OpenAI, &p, &Usage::default(), &CostOpts::default());
        assert_eq!(c, Cost::ZERO);
    }

    #[test]
    fn cost_display_matches_expectations() {
        let p = gpt4o_pricing();
        let u = Usage { input_tokens: 1_000_000, output_tokens: 500_000, ..Usage::default() };
        let c = compute_cost(&Provider::OpenAI, &p, &u, &CostOpts::default());
        // 1M * 2500 + 500k * 10000 = 2_500_000_000 + 5_000_000_000 = 7_500_000_000
        assert_eq!(c.to_dollars_f64(), 7.5);
        assert_eq!(c.to_string(), "$7.50000000");
    }

    #[test]
    fn gemini_thoughts_included_in_candidates_quirk() {
        // On Gemini AI Studio, candidatesTokenCount already includes
        // thinking tokens.  The Usage struct models this by putting
        // thinking tokens in reasoning_tokens and only the non-thinking
        // output in output_tokens.  compute_cost bills both separately.
        let p = ModelPricing {
            input_cost_per_token: 100,
            output_cost_per_token: 400,
            ..ModelPricing::zeroed()
        };
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 200,
            reasoning_tokens: 300,
            ..Usage::default()
        };
        let c = compute_cost(&Provider::Gemini, &p, &u, &CostOpts::default());
        // 1000 * 100 + 200 * 400 + 300 * 400 (reasoning falls back to output)
        // = 100_000 + 80_000 + 120_000 = 300_000
        assert_eq!(c, Cost::from_nanodollars(300_000));
    }
}
