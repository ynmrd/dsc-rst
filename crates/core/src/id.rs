use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Milliseconds between the Unix epoch and the Discord epoch (2015-01-01T00:00:00Z).
pub const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// A Discord snowflake. Serialized as a string (as Discord does), deserialized from
/// either a string or an integer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub u64);

pub type GuildId = Id;
pub type ChannelId = Id;
pub type MessageId = Id;
pub type UserId = Id;
pub type RoleId = Id;
pub type EmojiId = Id;
pub type ApplicationId = Id;

impl Id {
    pub const fn new(v: u64) -> Self {
        Id(v)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Unix timestamp (ms) encoded in the snowflake.
    pub const fn timestamp_ms(self) -> u64 {
        (self.0 >> 22) + DISCORD_EPOCH_MS
    }

    /// Smallest snowflake that could have been generated at `unix_ms`; useful for
    /// `before`/`after` pagination by time.
    pub const fn from_timestamp_ms(unix_ms: u64) -> Self {
        Id(unix_ms.saturating_sub(DISCORD_EPOCH_MS) << 22)
    }

    pub fn datetime(self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(self.timestamp_ms() as i64).unwrap_or_default()
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim().parse().map(Id)
    }
}

impl From<u64> for Id {
    fn from(v: u64) -> Self {
        Id(v)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Id;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a snowflake as string or integer")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Id, E> {
                Ok(Id(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Id, E> {
                u64::try_from(v).map(Id).map_err(E::custom)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Id, E> {
                v.parse().map(Id).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_string_and_number() {
        let a: Id = serde_json::from_str("\"175928847299117063\"").unwrap();
        let b: Id = serde_json::from_str("175928847299117063").unwrap();
        assert_eq!(a, b);
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"175928847299117063\"");
    }

    #[test]
    fn timestamp_matches_docs_example() {
        // Example from the Discord developer docs: 175928847299117063 -> 2016-04-30T11:18:25.796Z
        let id = Id(175_928_847_299_117_063);
        assert_eq!(id.timestamp_ms(), 1_462_015_105_796);
        assert_eq!(Id::from_timestamp_ms(id.timestamp_ms()).timestamp_ms(), id.timestamp_ms());
    }

    #[test]
    fn option_id_null() {
        let v: Option<Id> = serde_json::from_str("null").unwrap();
        assert!(v.is_none());
    }
}
