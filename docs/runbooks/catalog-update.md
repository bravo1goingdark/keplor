# Catalog Update (Model-Cost)

**Status: stub — depends on hot-reloadable Catalog.** Today the
LiteLLM pricing JSON is bundled at compile time via `include_bytes!` in
`crates/keplor-pricing/src/catalog.rs`. The `Pipeline` holds a plain
`Arc<Catalog>` (see `pipeline.rs:65`), not an `ArcSwap`, so the running
process cannot pick up a new catalog without a redeploy. The
`Catalog::load_from_disk` and (feature-gated) `Catalog::fetch_latest`
APIs exist but are unused on the ingest hot path.

The manual workaround is **rebuild and redeploy** any time you need
fresher prices, a new model, or a corrected cache-tier rate.

## Trigger

- New model launched by a provider that we ingest events for, and
  cost is showing as `0` on the dashboard.
- LiteLLM publishes a corrected price (most often: caching tiers,
  batch-mode discounts, regional surcharges).
- Customer-reported cost mismatch vs. the provider's own bill.
- Quarterly catalog refresh per the `PRICING_CATALOG_DATE` constant
  in `catalog.rs` getting stale (>90 days).

## Verify

1. Confirm the running version of the bundled snapshot:
   ```
   curl -s localhost:8080/health | jq '.pricing_catalog_version, .pricing_catalog_date'
   # or fall back to:
   ./keplor --version  # build sha encodes the catalog snapshot indirectly
   grep -E "PRICING_CATALOG_(VERSION|DATE)" \
     crates/keplor-pricing/src/catalog.rs
   ```
2. Spot-check the offending model — query a recent event and confirm
   `cost_nanodollars` is zero or wrong:
   ```
   curl -s -H "Authorization: Bearer $KEY" \
     "http://localhost:8080/v1/events?model=$MODEL&limit=5" | jq '.events[] | {model, cost_nanodollars}'
   ```
3. Confirm the model is actually in upstream LiteLLM JSON before
   rebuilding (otherwise the rebuild won't help):
   ```
   curl -s https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window_backup.json \
     | jq --arg m "$MODEL" '.[$m]'
   ```

## Fix

1. Pull the latest LiteLLM snapshot into the bundle path:
   ```
   cd crates/keplor-pricing
   curl -fsSL -o data/model_prices_and_context_window.json \
     https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window_backup.json
   ```
2. Update both constants in `crates/keplor-pricing/src/catalog.rs` —
   they are the only two strings the binary surfaces about catalog
   provenance:
   ```rust
   pub const PRICING_CATALOG_VERSION: &str = "<new-litellm-commit-sha>";
   pub const PRICING_CATALOG_DATE:    &str = "YYYY-MM-DD";
   ```
3. Run the catalog tests — they pin `gpt-4o`, `claude-sonnet-4`,
   `gemini-2.0-flash`, plus Anthropic caching and OpenAI batch fields.
   New entries that lack `input_cost_per_token` are silently skipped at
   load, so failing tests usually mean the snapshot drifted in shape:
   ```
   cargo test -p keplor-pricing
   ```
4. Build and roll the new binary on every keplor instance. Note that
   rebuild is mandatory — there is no runtime swap path:
   ```
   cargo build --release --target x86_64-unknown-linux-musl \
     -p keplor-cli --features mimalloc
   sudo systemctl restart keplor
   ```
5. Verify the version bumped on the live process:
   ```
   curl -s localhost:8080/health | jq '.pricing_catalog_version, .pricing_catalog_date'
   ```
6. Spot-check the previously-broken model:
   ```
   # Submit a known-cost event, query it back, compare vs provider bill.
   ```
7. Past events with wrong cost are **not** re-priced — keplor records
   `cost_nanodollars` at ingest time and never recomputes. If the gap
   matters for billing, exclude the affected window from the customer
   report or open a back-fill ticket.

**TODO: requires `Arc<ArcSwap<Catalog>>` plumbing in `Pipeline`** plus
either a SIGHUP path that calls `Catalog::load_from_disk(catalog_path)`
or a periodic refresh task that calls `Catalog::fetch_latest`. Once
shipped, this runbook collapses to: drop new JSON in
`/etc/keplor/pricing.json`, send SIGHUP, watch metrics, no restart.

## Post-mortem template

1. Timeline (UTC)
2. Detection: dashboard zero-cost spike, customer email, scheduled refresh
3. Customer impact: under/over-billing window, dollar magnitude
4. Root cause: catalog age, missing model, LiteLLM error
5. Resolution: snapshot pulled, redeployed, verified live
6. Action items: refresh cadence, alert on `cost_nanodollars=0` rate,
   ship runtime catalog hot-reload
