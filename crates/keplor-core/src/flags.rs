//! [`EventFlags`] — per-event boolean signals packed into 32 bits.
//!
//! Lives in a dedicated module because `bitflags!` expands into a lot of
//! trait impls we don't want mixing with the event struct.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    /// Per-event boolean signals.  Additive; unknown bits are reserved.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct EventFlags: u32 {
        /// Response body was streamed (SSE / event-stream / NDJSON).
        const STREAMING         = 1 << 0;
        /// Response contains one or more tool / function calls.
        const TOOL_CALLS        = 1 << 1;
        /// Response contains reasoning / thinking tokens.
        const REASONING         = 1 << 2;
        /// Stream ended prematurely (client disconnect, upstream abort).
        const STREAM_INCOMPLETE = 1 << 3;
        /// Provider cache was used (Anthropic cache_read, OpenAI cached).
        const CACHED_USED       = 1 << 4;
        /// Request was blocked by a server-side budget rule (phase 10+).
        const BUDGET_BLOCKED    = 1 << 5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        assert_eq!(EventFlags::default(), EventFlags::empty());
    }

    #[test]
    fn combine_and_check() {
        let f = EventFlags::STREAMING | EventFlags::TOOL_CALLS;
        assert!(f.contains(EventFlags::STREAMING));
        assert!(f.contains(EventFlags::TOOL_CALLS));
        assert!(!f.contains(EventFlags::REASONING));
    }

    #[test]
    fn serde_roundtrip() {
        let f = EventFlags::STREAMING | EventFlags::REASONING | EventFlags::CACHED_USED;
        let j = serde_json::to_string(&f).unwrap();
        let back: EventFlags = serde_json::from_str(&j).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn unknown_bit_positions_are_reserved() {
        // Asserts no accidental collision with bit positions 16+ that
        // future flags may occupy.
        let f = EventFlags::all();
        assert!(f.bits() < (1 << 16), "low bits only, upper 16 reserved");
    }
}
