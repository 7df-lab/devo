//! Opaque identifier newtypes for the native protocol surface.
//!
//! IDs are opaque strings on the wire. Newly created resources use a prefixed
//! form (`ses_` / `turn_` / `item_` / ...); legacy bare UUIDs from pre-v2
//! rollouts remain valid, must round-trip unchanged, and are accepted anywhere
//! an ID is expected. Clients must not parse IDs.
//!
//! Internally each ID is an interned handle (`Copy`). Pass them by value;
//! do not clone.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use ts_rs::TS;
use uuid::Uuid;

struct Interner {
    to_id: HashMap<&'static str, u32>,
    to_str: Vec<&'static str>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(|| {
        Mutex::new(Interner {
            to_id: HashMap::new(),
            to_str: Vec::new(),
        })
    })
}

fn intern(s: &str) -> u32 {
    let mut pool = interner().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(&id) = pool.to_id.get(s) {
        return id;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    let id = pool.to_str.len() as u32;
    pool.to_id.insert(leaked, id);
    pool.to_str.push(leaked);
    id
}

fn interned(id: u32) -> &'static str {
    let pool = interner().lock().unwrap_or_else(|error| error.into_inner());
    pool.to_str[id as usize]
}

/// Shared behavior for Native opaque IDs (`SessionId`, `TurnId`, `ItemId`, …).
///
/// All implementors are interned and [`Copy`]. Functions should take `T` by
/// value (or `impl OpaqueId` when the concrete type does not matter).
pub trait OpaqueId:
    Copy + Eq + Ord + Hash + fmt::Display + fmt::Debug + AsRef<str> + 'static
{
    /// Interned textual form. Stable for the process lifetime.
    fn as_str(&self) -> &'static str;
}

macro_rules! define_opaque_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
        pub struct $name(u32);

        impl JsonSchema for $name {
            fn schema_name() -> String {
                String::from(stringify!($name))
            }

            fn json_schema(
                generator: &mut schemars::r#gen::SchemaGenerator,
            ) -> schemars::schema::Schema {
                String::json_schema(generator)
            }
        }

        impl TS for $name {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;

            fn name(_: &ts_rs::Config) -> String {
                String::from(stringify!($name))
            }

            fn inline(cfg: &ts_rs::Config) -> String {
                Self::name(cfg)
            }

            fn decl(_: &ts_rs::Config) -> String {
                String::from(concat!("type ", stringify!($name), " = string;"))
            }
        }

        impl $name {
            /// Generates a new prefixed ID (`<prefix><uuid-v7>`).
            pub fn new() -> Self {
                Self(intern(&format!("{}{}", $prefix, Uuid::now_v7().simple())))
            }

            /// Wraps a legacy bare UUID from pre-v2 rollout files, preserving
            /// the original textual form so it round-trips unchanged.
            pub fn from_legacy_uuid(value: Uuid) -> Self {
                Self(intern(&value.to_string()))
            }

            /// Wraps an ID string received over the wire without interpreting it.
            pub fn from_string(value: String) -> Self {
                Self(intern(&value))
            }

            pub fn as_str(&self) -> &'static str {
                interned(self.0)
            }
        }

        impl OpaqueId for $name {
            fn as_str(&self) -> &'static str {
                interned(self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.as_str().cmp(other.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Ok(Self(intern(&value)))
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_legacy_uuid(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(intern(&value))
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(intern(value))
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(intern(s)))
            }
        }
    };
}

define_opaque_id!(SessionId, "ses_");
define_opaque_id!(TurnId, "turn_");
define_opaque_id!(ItemId, "item_");
define_opaque_id!(GoalId, "goal_");
define_opaque_id!(EventId, "evt_");
define_opaque_id!(RunId, "run_");
define_opaque_id!(SubscriptionId, "sub_");
define_opaque_id!(QueueItemId, "qit_");
define_opaque_id!(RestorePlanId, "rpl_");
define_opaque_id!(JobId, "job_");

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn new_ids_carry_their_prefix() {
        assert!(SessionId::new().as_str().starts_with("ses_"));
        assert!(TurnId::new().as_str().starts_with("turn_"));
        assert!(ItemId::new().as_str().starts_with("item_"));
        assert!(GoalId::new().as_str().starts_with("goal_"));
    }

    #[test]
    fn legacy_bare_uuid_round_trips_unchanged() {
        let uuid = Uuid::now_v7();
        let id = SessionId::from_legacy_uuid(uuid);
        assert_eq!(id.as_str(), uuid.to_string());
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{uuid}\""));
        let back: SessionId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn ids_are_copy_and_intern_equal() {
        let left = SessionId::from_string("ses_shared".into());
        let right = SessionId::from_string("ses_shared".into());
        let copied = left;
        assert_eq!(left, right);
        assert_eq!(left, copied);
        assert_eq!(left.as_str(), "ses_shared");
    }

    #[test]
    fn opaque_id_trait_is_implemented() {
        fn as_text(id: impl OpaqueId) -> &'static str {
            id.as_str()
        }
        let session = SessionId::from_string("ses_trait".into());
        assert_eq!(as_text(session), "ses_trait");
        assert_eq!(
            as_text(TurnId::from_string("turn_trait".into())),
            "turn_trait"
        );
    }
}
