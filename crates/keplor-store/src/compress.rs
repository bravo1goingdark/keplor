//! [`ZstdCoder`] — zstd compression with optional trained-dictionary support.
//!
//! Default: level 3, no dict.  Per-`(provider, component_type)` trained
//! dicts are loaded from `zstd_dicts` at startup and swapped via `Arc`.
//! Dict training itself lands in phase 8; this module provides the
//! infrastructure.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::StoreError;

/// Key for looking up a trained dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DictKey {
    /// Provider id string (e.g. `"openai"`).
    pub provider: String,
    /// Component type (e.g. `"system_prompt"`).
    pub component_type: String,
}

/// Zstd encoder/decoder with optional per-key trained dictionaries.
///
/// Dictionaries are stored as raw bytes; a fresh `Compressor` /
/// `Decompressor` is created per operation to stay `Send + Sync`.
#[derive(Debug, Clone)]
pub struct ZstdCoder {
    level: i32,
    dicts: Arc<HashMap<DictKey, Arc<Vec<u8>>>>,
}

impl ZstdCoder {
    /// Create a coder with the default compression level and no dicts.
    #[must_use]
    pub fn new() -> Self {
        Self { level: 3, dicts: Arc::new(HashMap::new()) }
    }

    /// Create a coder with a specific compression level.
    #[must_use]
    pub fn with_level(level: i32) -> Self {
        Self { level, dicts: Arc::new(HashMap::new()) }
    }

    /// Compress bytes, using a trained dict if one exists for `key`.
    pub fn compress(&self, data: &[u8], key: Option<&DictKey>) -> Result<Vec<u8>, StoreError> {
        let result = if let Some(dict_bytes) = key.and_then(|k| self.dicts.get(k)) {
            let mut comp = zstd::bulk::Compressor::with_dictionary(self.level, dict_bytes)
                .map_err(|e| StoreError::Compression(e.to_string()))?;
            comp.compress(data)
        } else {
            zstd::bulk::compress(data, self.level)
        };
        result.map_err(|e| StoreError::Compression(e.to_string()))
    }

    /// Maximum decompressed size (100 MB) — prevents OOM from malicious
    /// zstd headers claiming extreme uncompressed sizes.
    const MAX_DECOMPRESSED: usize = 100 * 1024 * 1024;

    /// Decompress bytes, using a trained dict if one exists for `key`.
    pub fn decompress(&self, data: &[u8], key: Option<&DictKey>) -> Result<Vec<u8>, StoreError> {
        let capacity = zstd::bulk::Decompressor::upper_bound(data)
            .unwrap_or(data.len() * 4)
            .clamp(64, Self::MAX_DECOMPRESSED);
        let result = if let Some(dict_bytes) = key.and_then(|k| self.dicts.get(k)) {
            let mut dec = zstd::bulk::Decompressor::with_dictionary(dict_bytes)
                .map_err(|e| StoreError::Compression(e.to_string()))?;
            dec.decompress(data, capacity)
        } else {
            zstd::bulk::decompress(data, capacity)
        };
        result.map_err(|e| StoreError::Compression(e.to_string()))
    }

    /// Check whether a trained dictionary exists for this key.
    #[must_use]
    pub fn has_dict(&self, key: &DictKey) -> bool {
        self.dicts.contains_key(key)
    }

    /// Register a trained dictionary.
    pub fn register_dict(&mut self, key: DictKey, dict_bytes: Vec<u8>) -> Result<(), StoreError> {
        Arc::get_mut(&mut self.dicts)
            .ok_or_else(|| StoreError::Compression("dicts arc is shared".into()))?
            .insert(key, Arc::new(dict_bytes));
        Ok(())
    }
}

impl Default for ZstdCoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_no_dict() {
        let coder = ZstdCoder::new();
        let data = b"hello world, this is a test of zstd compression.";
        let compressed = coder.compress(data, None).unwrap();
        let decompressed = coder.decompress(&compressed, None).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn roundtrip_with_dict() {
        let mut coder = ZstdCoder::new();
        let samples: Vec<&[u8]> = (0..100)
            .map(|i| match i % 3 {
                0 => &b"You are a helpful assistant that answers questions."[..],
                1 => &b"You are an AI assistant. Be concise and helpful."[..],
                _ => &b"System: respond in JSON format with structured data."[..],
            })
            .collect();
        let dict_bytes = zstd::dict::from_samples(&samples, 4096).unwrap();
        let key = DictKey { provider: "openai".into(), component_type: "system_prompt".into() };
        coder.register_dict(key.clone(), dict_bytes).unwrap();

        let data = b"You are a helpful assistant that answers questions.";
        let compressed = coder.compress(data, Some(&key)).unwrap();
        let decompressed = coder.decompress(&compressed, Some(&key)).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn empty_data() {
        let coder = ZstdCoder::new();
        let compressed = coder.compress(b"", None).unwrap();
        let decompressed = coder.decompress(&compressed, None).unwrap();
        assert!(decompressed.is_empty());
    }
}
