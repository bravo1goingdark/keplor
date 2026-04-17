//! Integration tests: fixture-based cost verification and property tests.

use keplor_core::{Cost, Provider, Usage};
use keplor_pricing::compute::{compute_cost, CacheTtl, ContextBucket, CostOpts};
use keplor_pricing::{Catalog, ModelKey};
use std::sync::Arc;

use proptest::prelude::*;

// ── Fixture-based tests ───────────────────────────────────────────────

fn provider_from_str(s: &str) -> Provider {
    match s {
        "openai" => Provider::OpenAI,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "bedrock" => Provider::Bedrock,
        "azure" => Provider::AzureOpenAI,
        "mistral" => Provider::Mistral,
        "groq" => Provider::Groq,
        "xai" => Provider::XAi,
        "deepseek" => Provider::DeepSeek,
        "cohere" => Provider::Cohere,
        "ollama" => Provider::Ollama,
        _ => Provider::OpenAICompatible { base_url: Arc::from(s) },
    }
}

fn parse_opts(obj: &serde_json::Value) -> CostOpts {
    let is_batch = obj.get("is_batch").and_then(|v| v.as_bool()).unwrap_or(false);
    let cache_ttl = match obj.get("cache_ttl").and_then(|v| v.as_str()) {
        Some("hours1") => CacheTtl::Hours1,
        _ => CacheTtl::Minutes5,
    };
    let context_bucket = match obj.get("context_bucket").and_then(|v| v.as_str()) {
        Some("above200k") => ContextBucket::Above200k,
        _ => ContextBucket::Standard,
    };
    CostOpts { is_batch, cache_ttl, context_bucket, ..CostOpts::default() }
}

macro_rules! fixture_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            let json_str = include_str!(concat!("fixtures/usage/", $file));
            let v: serde_json::Value = serde_json::from_str(json_str).unwrap();

            let provider = provider_from_str(v["provider"].as_str().unwrap());
            let model = v["model"].as_str().unwrap();
            let expected = v["expected_cost_nanodollars"].as_i64().unwrap();

            let usage_val = &v["usage"];
            let usage = Usage {
                input_tokens: usage_val.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                    as u32,
                output_tokens: usage_val.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                    as u32,
                cache_read_input_tokens: usage_val
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                cache_creation_input_tokens: usage_val
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                reasoning_tokens: usage_val
                    .get("reasoning_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                audio_input_tokens: usage_val
                    .get("audio_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                video_seconds: usage_val.get("video_seconds").and_then(|v| v.as_u64()).unwrap_or(0)
                    as u32,
                search_queries: usage_val
                    .get("search_queries")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                ..Usage::default()
            };

            let opts = parse_opts(&v["opts"]);

            let catalog = Catalog::load_bundled().unwrap();
            let key = ModelKey::new(model);
            let pricing = catalog.lookup(&key).unwrap_or_else(|| {
                panic!("model '{}' not found in catalog", model);
            });

            let cost = compute_cost(&provider, pricing, &usage, &opts);
            assert_eq!(
                cost,
                Cost::from_nanodollars(expected),
                "fixture {}: expected {} got {}",
                $file,
                expected,
                cost.nanodollars()
            );
        }
    };
}

fixture_test!(fixture_openai_gpt4o, "openai_gpt4o.json");
fixture_test!(fixture_openai_gpt4o_cached, "openai_gpt4o_cached.json");
fixture_test!(fixture_openai_gpt4o_batch, "openai_gpt4o_batch.json");
fixture_test!(fixture_anthropic_claude_sonnet_cached, "anthropic_claude_sonnet_cached.json");
fixture_test!(fixture_anthropic_cache_write_above_1hr, "anthropic_cache_write_above_1hr.json");
fixture_test!(fixture_bedrock_claude_cached, "bedrock_claude_cached.json");
fixture_test!(fixture_gemini_flash_with_video, "gemini_flash_with_video.json");

// ── Property tests ────────────────────────────────────────────────────

fn arb_usage() -> impl Strategy<Value = Usage> {
    (
        0u32..100_000,
        0u32..100_000,
        0u32..100_000,
        0u32..100_000,
        0u32..100_000,
        0u32..100_000,
        0u32..1_000,
        0u32..100,
    )
        .prop_map(
            |(
                input,
                output,
                cache_read,
                cache_create,
                reasoning,
                audio_in,
                video_sec,
                search_q,
            )| {
                Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_input_tokens: cache_read,
                    cache_creation_input_tokens: cache_create,
                    reasoning_tokens: reasoning,
                    audio_input_tokens: audio_in,
                    video_seconds: video_sec,
                    search_queries: search_q,
                    ..Usage::default()
                }
            },
        )
}

fn arb_provider() -> impl Strategy<Value = Provider> {
    prop_oneof![
        Just(Provider::OpenAI),
        Just(Provider::Anthropic),
        Just(Provider::Gemini),
        Just(Provider::Bedrock),
        Just(Provider::AzureOpenAI),
        Just(Provider::Mistral),
    ]
}

proptest! {
    /// Cost is non-negative for any valid usage (all rates are non-negative).
    #[test]
    fn cost_is_non_negative(
        usage in arb_usage(),
        provider in arb_provider(),
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            cache_read_input_token_cost: Some(1_250),
            cache_creation_input_token_cost: Some(3_750),
            output_cost_per_reasoning_token: Some(15_000),
            input_cost_per_audio_token: Some(100_000),
            input_cost_per_video_per_second: Some(1_000_000),
            search_context_cost_per_query: Some(10_000_000),
            ..ModelPricing::zeroed()
        };
        let cost = compute_cost(&provider, &pricing, &usage, &CostOpts::default());
        prop_assert!(cost.nanodollars() >= 0, "cost must be non-negative, got {}", cost);
    }

    /// Monotonic in input_tokens: more input → cost ≥ previous.
    #[test]
    fn monotonic_in_input_tokens(
        base_input in 0u32..100_000,
        extra in 1u32..100_000,
        output in 0u32..100_000,
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            ..ModelPricing::zeroed()
        };
        let u1 = Usage { input_tokens: base_input, output_tokens: output, ..Usage::default() };
        let u2 = Usage {
            input_tokens: base_input.saturating_add(extra),
            output_tokens: output,
            ..Usage::default()
        };
        let c1 = compute_cost(&Provider::OpenAI, &pricing, &u1, &CostOpts::default());
        let c2 = compute_cost(&Provider::OpenAI, &pricing, &u2, &CostOpts::default());
        prop_assert!(c2 >= c1, "input monotonicity violated: {} > {}", c1, c2);
    }

    /// Monotonic in output_tokens.
    #[test]
    fn monotonic_in_output_tokens(
        input in 0u32..100_000,
        base_output in 0u32..100_000,
        extra in 1u32..100_000,
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            ..ModelPricing::zeroed()
        };
        let u1 = Usage { input_tokens: input, output_tokens: base_output, ..Usage::default() };
        let u2 = Usage {
            input_tokens: input,
            output_tokens: base_output.saturating_add(extra),
            ..Usage::default()
        };
        let c1 = compute_cost(&Provider::OpenAI, &pricing, &u1, &CostOpts::default());
        let c2 = compute_cost(&Provider::OpenAI, &pricing, &u2, &CostOpts::default());
        prop_assert!(c2 >= c1, "output monotonicity violated: {} > {}", c1, c2);
    }

    /// Monotonic in reasoning_tokens.
    #[test]
    fn monotonic_in_reasoning_tokens(
        input in 0u32..100_000,
        output in 0u32..100_000,
        base_reason in 0u32..100_000,
        extra in 1u32..100_000,
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            output_cost_per_reasoning_token: Some(15_000),
            ..ModelPricing::zeroed()
        };
        let u1 = Usage {
            input_tokens: input, output_tokens: output,
            reasoning_tokens: base_reason, ..Usage::default()
        };
        let u2 = Usage {
            input_tokens: input, output_tokens: output,
            reasoning_tokens: base_reason.saturating_add(extra), ..Usage::default()
        };
        let c1 = compute_cost(&Provider::OpenAI, &pricing, &u1, &CostOpts::default());
        let c2 = compute_cost(&Provider::OpenAI, &pricing, &u2, &CostOpts::default());
        prop_assert!(c2 >= c1, "reasoning monotonicity violated: {} > {}", c1, c2);
    }

    /// Monotonic in cache_read_input_tokens for Anthropic (additive).
    #[test]
    fn monotonic_in_cache_read_anthropic(
        input in 0u32..100_000,
        output in 0u32..100_000,
        base_cr in 0u32..100_000,
        extra in 1u32..100_000,
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 3_000,
            output_cost_per_token: 15_000,
            cache_read_input_token_cost: Some(300),
            ..ModelPricing::zeroed()
        };
        let u1 = Usage {
            input_tokens: input, output_tokens: output,
            cache_read_input_tokens: base_cr, ..Usage::default()
        };
        let u2 = Usage {
            input_tokens: input, output_tokens: output,
            cache_read_input_tokens: base_cr.saturating_add(extra), ..Usage::default()
        };
        let c1 = compute_cost(&Provider::Anthropic, &pricing, &u1, &CostOpts::default());
        let c2 = compute_cost(&Provider::Anthropic, &pricing, &u2, &CostOpts::default());
        prop_assert!(c2 >= c1, "cache_read monotonicity violated: {} > {}", c1, c2);
    }

    /// Batch cost ≤ standard cost when batch rates exist.
    #[test]
    fn batch_cheaper_than_standard(
        input in 0u32..100_000,
        output in 0u32..100_000,
    ) {
        use keplor_pricing::model::ModelPricing;
        let pricing = ModelPricing {
            input_cost_per_token: 2_500,
            output_cost_per_token: 10_000,
            input_cost_per_token_batches: Some(1_250),
            output_cost_per_token_batches: Some(5_000),
            ..ModelPricing::zeroed()
        };
        let u = Usage { input_tokens: input, output_tokens: output, ..Usage::default() };
        let standard = compute_cost(&Provider::OpenAI, &pricing, &u, &CostOpts::default());
        let batch = compute_cost(&Provider::OpenAI, &pricing, &u, &CostOpts { is_batch: true, ..CostOpts::default() });
        prop_assert!(batch <= standard, "batch should be <= standard: {} > {}", batch, standard);
    }
}
