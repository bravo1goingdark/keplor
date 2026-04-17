# Phase 2 — Pricing catalog and cost engine

**Status:** not started
**Depends on:** phase 1
**Unlocks:** phase 6

## Goal

Given a `(provider, model, usage)`, return a `Cost` in nanodollars with correct handling of cache, reasoning, batch, modality, and tier pricing.

## Prompt

Implement `keplor-pricing`.

1. Bundle a snapshot of LiteLLM's `model_prices_and_context_window.json` into the binary via `include_bytes!`. Source:
   ```
   https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/model_prices_and_context_window_backup.json
   ```
   Record the git commit SHA and fetch date in a `PRICING_CATALOG_VERSION` const.

2. Define `ModelPricing` struct covering every field from the research:
   - `input_cost_per_token`
   - `output_cost_per_token`
   - `cache_read_input_token_cost`
   - `cache_creation_input_token_cost`
   - `cache_creation_input_token_cost_above_1hr`
   - `cache_creation_input_token_cost_above_200k_tokens`
   - `output_cost_per_reasoning_token`
   - `input_cost_per_image`
   - `input_cost_per_audio_token`
   - `input_cost_per_video_per_second`
   - `input_cost_per_token_batches`
   - `output_cost_per_token_batches`
   - `search_context_cost_per_query`
   - `tool_use_system_prompt_tokens`
   - `max_input_tokens`, `max_output_tokens`
   - `supports_reasoning`, `supports_prompt_caching`, `supports_vision`
   - `deprecation_date`
   - `aliases: Vec<SmolStr>`
   - `inference_geo_multiplier: Option<f64>`
   - `litellm_provider`, `mode`

3. `Catalog` struct: `HashMap<ModelKey, ModelPricing>` where `ModelKey` is a normalized string (lowercased, provider-prefixed). Alias resolution: every `aliases` entry also indexes the canonical entry. Plus a prefix-match fallback: `gpt-4o-2024-08-06` → `gpt-4o`.

4. Loaders:
   - `Catalog::load_bundled()` — parse the bundled JSON.
   - `Catalog::load_from_disk(path)` — hot-reload override.
   - `Catalog::fetch_latest(url)` — background refresh. Failure falls back to bundled. Returns a new `Catalog` for the caller to swap via `arc-swap`.

5. Cost compute:
   ```rust
   pub fn compute_cost(
       pricing: &ModelPricing,
       usage: &Usage,
       opts: CostOpts,
   ) -> Cost
   ```
   where `CostOpts` carries: `is_batch`, `service_tier` (Standard/Flex/Priority), `inference_geo`, `cache_ttl` (5m/1h), `context_bucket` (≤200k / >200k). Must correctly:
   - Distinguish Anthropic (input_tokens excludes cache) from OpenAI (input_tokens includes cache_read) — look up by provider.
   - Apply cache-creation tier surcharges.
   - Apply batch discount when `is_batch`.
   - Apply geo multiplier for Anthropic when region is US and multiplier is set.
   - Bill `reasoning_tokens` at `output_cost_per_reasoning_token` if present, else at `output_cost_per_token`.
   - Bill image/audio/video modalities from their per-unit rates.

6. Doctests for every edge case from the research: Anthropic cache-write above-1h, OpenAI cached_tokens 50% discount on Responses, Bedrock cache_write, Gemini thoughts-included-in-candidates quirk.

7. An xtask `cargo xtask refresh-catalog` that downloads the latest JSON, runs the test suite against it, and updates the bundled snapshot + `PRICING_CATALOG_VERSION` in one commit.

## Acceptance criteria

- [ ] Property tests: `compute_cost` is monotonic in each token dimension (non-negative contribution)
- [ ] Unit tests using fixture usage frames from real provider responses (`tests/fixtures/usage/*.json`)
- [ ] `cargo test -p keplor-pricing` green
- [ ] Binary size contribution of the bundled catalog reported via `cargo bloat`
- [ ] `cargo clippy -p keplor-pricing -- -D warnings` green
