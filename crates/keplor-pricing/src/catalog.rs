//! [`Catalog`] — the in-memory pricing index with alias resolution and
//! date-suffix fallback lookup.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use smol_str::SmolStr;

use crate::error::PricingError;
use crate::model::{ModelPricing, RawEntry};

/// Bundled pricing JSON (LiteLLM `model_prices_and_context_window_backup.json`).
const BUNDLED_JSON: &[u8] = include_bytes!("../data/model_prices_and_context_window.json");

/// Git commit SHA of the bundled catalogue snapshot.
pub const PRICING_CATALOG_VERSION: &str = "44c992416cfab1d911299ed6d57fa6ad974af1a7";

/// Date the bundled catalogue was fetched.
pub const PRICING_CATALOG_DATE: &str = "2026-04-17";

/// Normalised model key used for catalog lookups.
///
/// Keys are **lowercased** and kept as-is from the LiteLLM JSON (which
/// may include a provider prefix like `"openai/"` or `"azure/"`).
///
/// # Examples
///
/// ```
/// use keplor_pricing::ModelKey;
/// let k = ModelKey::new("GPT-4o");
/// assert_eq!(k.as_str(), "gpt-4o");
/// let k2 = ModelKey::new("azure/GPT-4o");
/// assert_eq!(k2.as_str(), "azure/gpt-4o");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelKey(SmolStr);

impl ModelKey {
    /// Create a normalised key from an arbitrary model string.
    #[must_use]
    pub fn new(raw: &str) -> Self {
        Self(SmolStr::new(raw.to_ascii_lowercase()))
    }

    /// Wrap a pre-normalised (lowercase, trimmed) model string.
    ///
    /// Skips the `to_ascii_lowercase()` that [`ModelKey::new`] performs.
    #[must_use]
    pub fn from_normalized(s: SmolStr) -> Self {
        Self(s)
    }

    /// The normalised key as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// In-memory pricing catalogue.
///
/// Thread-safe (`Send + Sync`).  Callers typically hold an
/// `Arc<ArcSwap<Catalog>>` to swap in refreshed versions at runtime.
///
/// # Examples
///
/// ```
/// use keplor_pricing::{Catalog, ModelKey};
///
/// let catalog = Catalog::load_bundled().expect("bundled catalog parses");
/// let key = ModelKey::new("gpt-4o");
/// let pricing = catalog.lookup(&key).expect("gpt-4o exists");
/// assert!(pricing.input_cost_per_token > 0);
/// ```
#[derive(Debug)]
pub struct Catalog {
    entries: HashMap<ModelKey, Arc<ModelPricing>>,
}

impl Catalog {
    /// Number of distinct model keys in the index (including aliases).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog has zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Parse the catalogue from LiteLLM JSON bytes.
    fn from_json(bytes: &[u8]) -> Result<Self, PricingError> {
        let raw: HashMap<String, serde_json::Value> = serde_json::from_slice(bytes)
            .map_err(|e| PricingError::Parse { reason: e.to_string() })?;

        let mut entries: HashMap<ModelKey, Arc<ModelPricing>> = HashMap::new();

        for (key, value) in &raw {
            if key == "sample_spec" {
                continue;
            }
            // Image-generation / embedding-only entries don't have the
            // fields we need — try-parse and skip on failure.
            let entry: RawEntry = match serde_json::from_value(value.clone()) {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!(key, error = %e, "skipping unparseable catalog entry");
                    continue;
                },
            };

            // Skip entries without any cost data — they're capability-only
            // stubs or non-chat modes we don't price.
            if entry.input_cost_per_token.is_none() && entry.output_cost_per_token.is_none() {
                continue;
            }

            let pricing = Arc::new(entry.into_pricing());
            let canonical = ModelKey::new(key);

            // Index under the full key.
            entries.insert(canonical.clone(), Arc::clone(&pricing));

            // If the key has a provider prefix (`openai/gpt-4o`), also
            // index the unprefixed form when it doesn't collide.
            if let Some((_prefix, suffix)) = key.split_once('/') {
                let short = ModelKey::new(suffix);
                entries.entry(short).or_insert_with(|| Arc::clone(&pricing));
            }
        }

        Ok(Self { entries })
    }

    /// Load the catalogue bundled into the binary at compile time.
    ///
    /// # Errors
    ///
    /// Returns [`PricingError::Parse`] if the bundled JSON is somehow
    /// corrupt (should never happen in a release build).
    ///
    /// # Examples
    ///
    /// ```
    /// let cat = keplor_pricing::Catalog::load_bundled().unwrap();
    /// assert!(!cat.is_empty());
    /// ```
    pub fn load_bundled() -> Result<Self, PricingError> {
        Self::from_json(BUNDLED_JSON)
    }

    /// Load a catalogue from a JSON file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`PricingError::Io`] on read failure or
    /// [`PricingError::Parse`] on deserialisation failure.
    pub fn load_from_disk(path: &Path) -> Result<Self, PricingError> {
        let bytes = std::fs::read(path)
            .map_err(|e| PricingError::Io { path: path.to_path_buf(), source: e })?;
        Self::from_json(&bytes)
    }

    /// Download a fresh catalogue from a remote URL.
    ///
    /// Returns a **new** `Catalog`; the caller is responsible for
    /// swapping it in (e.g. via `arc-swap`).  On network failure returns
    /// an error — callers should fall back to the previously loaded
    /// catalog.
    ///
    /// # Errors
    ///
    /// Returns [`PricingError::Fetch`] on HTTP failure or
    /// [`PricingError::Parse`] if the downloaded JSON is invalid.
    #[cfg(feature = "fetch")]
    pub async fn fetch_latest(url: &str) -> Result<Self, PricingError> {
        let bytes = reqwest::get(url)
            .await
            .map_err(|e| PricingError::Fetch { source: e })?
            .bytes()
            .await
            .map_err(|e| PricingError::Fetch { source: e })?;
        Self::from_json(&bytes)
    }

    /// Look up pricing by exact normalised key.
    #[must_use]
    pub fn get(&self, key: &ModelKey) -> Option<&Arc<ModelPricing>> {
        self.entries.get(key)
    }

    /// Look up pricing with **fallback**: exact match first, then strip
    /// a trailing date suffix (`-YYYY-MM-DD` or `-YYYYMMDD`), then
    /// try prepending the LiteLLM provider prefix.
    ///
    /// # Examples
    ///
    /// ```
    /// use keplor_pricing::{Catalog, ModelKey};
    ///
    /// let cat = Catalog::load_bundled().unwrap();
    /// // Date-suffix fallback: the versioned key falls back to the base.
    /// let versioned = ModelKey::new("gpt-4o-2024-08-06");
    /// assert!(cat.lookup(&versioned).is_some());
    /// ```
    #[must_use]
    pub fn lookup(&self, key: &ModelKey) -> Option<&Arc<ModelPricing>> {
        // 1. Exact match.
        if let Some(p) = self.entries.get(key) {
            return Some(p);
        }

        let s = key.as_str();

        // 2. Strip trailing date suffix.
        if let Some(base) = strip_date_suffix(s) {
            let fallback = ModelKey::new(base);
            if let Some(p) = self.entries.get(&fallback) {
                return Some(p);
            }
        }

        // 3. If no provider prefix, try common prefixes.
        if !s.contains('/') {
            for prefix in &[
                "openai",
                "anthropic",
                "gemini",
                "vertex_ai",
                "bedrock",
                "azure",
                "mistral",
                "groq",
                "xai",
                "deepseek",
                "cohere",
                "ollama",
            ] {
                let prefixed = ModelKey::new(&format!("{prefix}/{s}"));
                if let Some(p) = self.entries.get(&prefixed) {
                    return Some(p);
                }
                // Also try with date stripped.
                if let Some(base) = strip_date_suffix(prefixed.as_str()) {
                    let fb = ModelKey::new(base);
                    if let Some(p) = self.entries.get(&fb) {
                        return Some(p);
                    }
                }
            }
        }

        None
    }

    /// Look up or return an error.
    ///
    /// # Errors
    ///
    /// Returns [`PricingError::ModelNotFound`] when the key is absent.
    pub fn lookup_or_err(&self, key: &ModelKey) -> Result<&Arc<ModelPricing>, PricingError> {
        self.lookup(key).ok_or_else(|| PricingError::ModelNotFound { key: key.to_string() })
    }
}

/// Strip a trailing date-like suffix: `-YYYY-MM-DD` or `-YYYYMMDD`.
fn strip_date_suffix(s: &str) -> Option<&str> {
    // -YYYY-MM-DD (11 chars)
    if s.len() > 11 {
        let tail = &s[s.len() - 11..];
        if tail.starts_with('-')
            && tail[1..5].bytes().all(|b| b.is_ascii_digit())
            && tail.as_bytes()[5] == b'-'
            && tail[6..8].bytes().all(|b| b.is_ascii_digit())
            && tail.as_bytes()[8] == b'-'
            && tail[9..11].bytes().all(|b| b.is_ascii_digit())
        {
            return Some(&s[..s.len() - 11]);
        }
    }
    // -YYYYMMDD (9 chars)
    if s.len() > 9 {
        let tail = &s[s.len() - 9..];
        if tail.starts_with('-') && tail[1..].bytes().all(|b| b.is_ascii_digit()) {
            return Some(&s[..s.len() - 9]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_loads() {
        let cat = Catalog::load_bundled().unwrap();
        assert!(cat.len() > 100, "expected >100 entries, got {}", cat.len());
    }

    #[test]
    fn exact_lookup_gpt4o() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("gpt-4o");
        let p = cat.lookup(&key).expect("gpt-4o must exist");
        assert!(p.input_cost_per_token > 0);
        assert!(p.output_cost_per_token > 0);
    }

    #[test]
    fn case_insensitive_lookup() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("GPT-4o");
        assert!(cat.lookup(&key).is_some());
    }

    #[test]
    fn date_suffix_fallback() {
        let cat = Catalog::load_bundled().unwrap();
        let versioned = ModelKey::new("gpt-4o-2024-08-06");
        assert!(cat.lookup(&versioned).is_some(), "date-suffix fallback should resolve to gpt-4o");
    }

    #[test]
    fn provider_prefix_fallback() {
        let cat = Catalog::load_bundled().unwrap();
        // LiteLLM indexes Anthropic models under `claude-sonnet-4-20250514`
        // without prefix AND sometimes under `anthropic/claude-sonnet-4-20250514`.
        let key = ModelKey::new("claude-sonnet-4-20250514");
        assert!(cat.lookup(&key).is_some(), "should find claude sonnet 4");
    }

    #[test]
    fn unprefixed_also_indexed() {
        let cat = Catalog::load_bundled().unwrap();
        // `gemini/gemini-2.0-flash` should be findable via `gemini-2.0-flash`
        let key = ModelKey::new("gemini-2.0-flash");
        assert!(cat.lookup(&key).is_some(), "unprefixed gemini-2.0-flash");
    }

    #[test]
    fn not_found_returns_none() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("nonexistent-model-xyz");
        assert!(cat.lookup(&key).is_none());
    }

    #[test]
    fn strip_date_suffix_iso() {
        assert_eq!(strip_date_suffix("gpt-4o-2024-08-06"), Some("gpt-4o"));
        assert_eq!(strip_date_suffix("claude-3-5-sonnet-20241022"), Some("claude-3-5-sonnet"));
    }

    #[test]
    fn strip_date_suffix_none_for_short() {
        assert_eq!(strip_date_suffix("gpt-4o"), None);
        assert_eq!(strip_date_suffix("x"), None);
    }

    #[test]
    fn strip_date_preserves_model_families() {
        // gpt-4o-mini-2024-07-18 should strip to gpt-4o-mini, NOT gpt-4o
        assert_eq!(strip_date_suffix("gpt-4o-mini-2024-07-18"), Some("gpt-4o-mini"));
    }

    #[test]
    fn model_key_normalises() {
        let k = ModelKey::new("GPT-4O");
        assert_eq!(k.as_str(), "gpt-4o");
    }

    #[test]
    fn anthropic_caching_fields_present() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("claude-sonnet-4-20250514");
        let p = cat.lookup(&key).expect("claude sonnet 4 must exist");
        assert!(p.cache_creation_input_token_cost.is_some());
        assert!(p.cache_read_input_token_cost.is_some());
        assert!(p.cache_creation_input_token_cost_above_1hr.is_some());
    }

    #[test]
    fn openai_batch_fields_present() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("gpt-4o");
        let p = cat.lookup(&key).expect("gpt-4o must exist");
        assert!(p.input_cost_per_token_batches.is_some());
        assert!(p.output_cost_per_token_batches.is_some());
    }

    #[test]
    fn lookup_or_err_returns_error() {
        let cat = Catalog::load_bundled().unwrap();
        let key = ModelKey::new("nonexistent");
        let err = cat.lookup_or_err(&key).unwrap_err();
        assert!(matches!(err, PricingError::ModelNotFound { .. }));
    }
}
