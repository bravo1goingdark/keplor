//! [`PayloadRef`] — a compact pointer to a captured request or response
//! body plus enough metadata to decompress + verify it.
//!
//! The actual bytes live in one of three places:
//!
//! - Inline in the event itself (tiny responses, e.g. auth errors).
//! - A row in the `payload_blobs` SQLite table (the default).
//! - An external URL (S3 / object store when the local DB exceeds its
//!   retention budget).

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use url::Url;

/// Row id for a blob in the `payload_blobs` SQLite table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobId(pub u64);

/// Row id for a trained zstd dictionary in the `zstd_dicts` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DictId(pub u32);

/// Where the payload bytes live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayloadStorage {
    /// Bytes embedded directly (used for very small payloads only).
    Inline {
        /// The raw (possibly compressed) bytes.
        data: Bytes,
    },
    /// A row in the local `payload_blobs` table.
    Blob {
        /// Row id.
        id: BlobId,
    },
    /// An external object-store URL (S3, GCS, etc.).
    External {
        /// Fully-qualified URL.
        url: Url,
    },
}

/// How the bytes at a [`PayloadStorage`] location are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Compression {
    /// Uncompressed.
    None,
    /// Plain zstd (level stored elsewhere).
    ZstdRaw,
    /// zstd with a trained dictionary.
    ZstdDict {
        /// Id of the dictionary used to encode / decode.
        dict_id: DictId,
    },
}

/// A pointer to captured bytes: where they live, how they're encoded, and
/// what size they were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRef {
    /// SHA-256 of the **uncompressed** bytes — integrity check on read.
    #[serde(with = "crate::payload_ref::hex32")]
    pub sha256: [u8; 32],
    /// Storage backend holding the bytes.
    pub storage: PayloadStorage,
    /// On-wire / on-disk encoding.
    pub compression: Compression,
    /// Uncompressed size (used for budgeting + quick "is it worth
    /// decompressing?" decisions).
    pub uncompressed_size: u32,
    /// Bytes actually occupied by [`PayloadRef::storage`].
    pub compressed_size: u32,
}

impl PayloadRef {
    /// Zero-sized inline payload (used where a response body is absent,
    /// e.g. 204 No Content upstreams).
    #[must_use]
    pub fn empty_inline() -> Self {
        Self {
            sha256: [0u8; 32],
            storage: PayloadStorage::Inline { data: Bytes::new() },
            compression: Compression::None,
            uncompressed_size: 0,
            compressed_size: 0,
        }
    }
}

/// Serialise `[u8; 32]` as a lowercase hex string so stored JSON is
/// grep-able instead of a 32-element integer array.
pub(crate) mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let hex_str = String::deserialize(d)?;
        let v = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        let arr: [u8; 32] =
            v.try_into().map_err(|_| serde::de::Error::custom("expected 32-byte hex"))?;
        Ok(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inline_is_well_formed() {
        let p = PayloadRef::empty_inline();
        assert_eq!(p.uncompressed_size, 0);
        assert_eq!(p.compressed_size, 0);
        matches!(p.storage, PayloadStorage::Inline { .. });
        matches!(p.compression, Compression::None);
    }

    #[test]
    fn sha256_hex_roundtrip() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        let p = PayloadRef {
            sha256: bytes,
            storage: PayloadStorage::Blob { id: BlobId(99) },
            compression: Compression::ZstdRaw,
            uncompressed_size: 1024,
            compressed_size: 128,
        };
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\""));
        let back: PayloadRef = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn storage_variants_roundtrip() {
        let blob = PayloadStorage::Blob { id: BlobId(42) };
        let j = serde_json::to_string(&blob).unwrap();
        assert!(j.contains("\"blob\""));
        let back: PayloadStorage = serde_json::from_str(&j).unwrap();
        assert_eq!(blob, back);

        let ext = PayloadStorage::External { url: Url::parse("https://s3.example/k").unwrap() };
        let j = serde_json::to_string(&ext).unwrap();
        let back: PayloadStorage = serde_json::from_str(&j).unwrap();
        assert_eq!(ext, back);
    }

    #[test]
    fn compression_zstd_dict_tracks_id() {
        let c = Compression::ZstdDict { dict_id: DictId(7) };
        let j = serde_json::to_string(&c).unwrap();
        assert!(j.contains("\"zstd_dict\""));
        assert!(j.contains("\"dict_id\":7"));
    }
}
