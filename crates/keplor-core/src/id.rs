//! Strongly-typed identifiers.
//!
//! Every foreign key that crosses a crate boundary gets its own newtype so
//! the type system prevents passing a `UserId` where a `RouteId` is
//! expected.  Each type implements [`Display`] and [`FromStr`] — round
//! trip `to_string` ↔ `parse` is always lossless.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use ulid::Ulid;

use crate::CoreError;

/// The primary key of a captured LLM event.  Time-sortable (ULID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub Ulid);

impl EventId {
    /// Generate a fresh [`EventId`] using the current wall-clock time.
    #[must_use]
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// Access the underlying ULID.
    #[must_use]
    pub const fn as_ulid(&self) -> Ulid {
        self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for EventId {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s)
            .map(Self)
            .map_err(|_| CoreError::InvalidId { kind: "event", value: s.to_owned() })
    }
}

macro_rules! string_id {
    ($name:ident, $kind:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub SmolStr);

        impl $name {
            /// Borrow the inner string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if s.is_empty() {
                    Err(CoreError::InvalidId { kind: $kind, value: s.to_owned() })
                } else {
                    Ok(Self(SmolStr::from(s)))
                }
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(SmolStr::from(s))
            }
        }
    };
}

string_id!(UserId, "user", "Caller-supplied user identifier (e.g. `\"u_abc123\"`).");
string_id!(ApiKeyId, "api_key", "Stable identifier for the API key used on the request.");
string_id!(OrgId, "org", "Organisation grouping for cost attribution.");
string_id!(ProjectId, "project", "Project grouping nested under [`OrgId`].");
string_id!(RouteId, "route", "Logical route name (`\"chat\"`, `\"embeddings\"`, …).");
string_id!(
    ProviderId,
    "provider",
    "Stable storage key for a [`crate::Provider`] (`\"openai\"`, `\"anthropic\"`, …)."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_roundtrip() {
        let id = EventId::new();
        let s = id.to_string();
        let parsed: EventId = s.parse().expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn event_id_rejects_garbage() {
        assert!("not-a-ulid".parse::<EventId>().is_err());
        assert!("".parse::<EventId>().is_err());
    }

    #[test]
    fn string_ids_roundtrip() {
        let u: UserId = "u_abc123".parse().unwrap();
        assert_eq!(u.to_string(), "u_abc123");
        assert_eq!(u.as_str(), "u_abc123");

        let r: RouteId = "chat".parse().unwrap();
        assert_eq!(r.as_str(), "chat");
    }

    #[test]
    fn string_ids_reject_empty() {
        assert!("".parse::<UserId>().is_err());
        assert!("".parse::<OrgId>().is_err());
        assert!("".parse::<ApiKeyId>().is_err());
        assert!("".parse::<ProjectId>().is_err());
        assert!("".parse::<RouteId>().is_err());
        assert!("".parse::<ProviderId>().is_err());
    }

    #[test]
    fn string_ids_from_str_for_convenience() {
        let u = UserId::from("alice");
        assert_eq!(u.0, SmolStr::from("alice"));
    }

    #[test]
    fn serde_roundtrip_is_transparent() {
        let e = EventId::new();
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.starts_with('"'));
        let back: EventId = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);

        let u = UserId::from("alice");
        let j = serde_json::to_string(&u).unwrap();
        assert_eq!(j, "\"alice\"");
    }
}
