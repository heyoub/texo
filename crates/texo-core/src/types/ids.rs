//! Branded domain identifiers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::parse::{self, IdParseError};

macro_rules! id_newtype {
    ($name:ident, $prefix:literal) => {
        #[doc = concat!("Branded ", stringify!($name), " identifier.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent, deny_unknown_fields)]
        pub struct $name(String);

        impl $name {
            /// Construct from a validated string (internal use).
            pub(crate) fn new_unchecked(value: String) -> Self {
                Self(value)
            }

            /// Borrow the inner string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdParseError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                parse::expect_prefix(value, $prefix).map(Self::new_unchecked)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_from(value)
            }
        }
    };
}

id_newtype!(ClaimId, "claim_");
id_newtype!(SourceId, "src_");
id_newtype!(ConflictId, "conflict_");
id_newtype!(DocId, "doc_");

/// Workspace scope identifier (no required prefix — validated for non-empty safe chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// Construct a workspace id from a validated segment.
    pub fn new(value: impl Into<String>) -> Result<Self, IdParseError> {
        let value = value.into();
        parse::validate_workspace(&value)?;
        Ok(Self(value))
    }

    /// Borrow the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// BatPak scope string: `workspace:{id}`.
    pub fn scope(&self) -> String {
        format!("workspace:{}", self.0)
    }
}

impl AsRef<str> for WorkspaceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for WorkspaceId {
    type Error = IdParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Number of body-hash hex characters carried in a derived source id.
const SOURCE_ID_HASH_LEN: usize = 12;

/// Compute deterministic source id from body hash hex.
///
/// Requires at least [`SOURCE_ID_HASH_LEN`] hex characters of input; a shorter
/// hash would be silently truncated to a different, ambiguous id, so it is
/// rejected instead. Callers derive the input from [`blake3_bytes_hex`], which
/// always yields 64 characters, so this error path is unreachable in practice.
pub fn source_id_from_hash(body_hash_hex: &str) -> Result<SourceId, IdParseError> {
    let prefix = body_hash_hex
        .get(..SOURCE_ID_HASH_LEN)
        .ok_or(IdParseError::HashTooShort {
            min: SOURCE_ID_HASH_LEN,
        })?;
    Ok(SourceId::new_unchecked(format!("src_{prefix}")))
}

/// Compute deterministic claim id.
pub fn claim_id_from_parts(
    source_id: &SourceId,
    line_start: u32,
    normalized_text: &str,
) -> ClaimId {
    let material = format!("{}{}{normalized_text}", source_id.as_str(), line_start);
    let hash = blake3_hash_hex(&material);
    ClaimId::new_unchecked(format!("claim_{}", &hash[..12]))
}

/// Compute deterministic conflict id (ordered pair).
pub fn conflict_id_from_pair(a: &ClaimId, b: &ClaimId) -> ConflictId {
    let (left, right) = if a.as_str() <= b.as_str() {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    let hash = blake3_hash_hex(&format!("{left}{right}"));
    ConflictId::new_unchecked(format!("conflict_{}", &hash[..12]))
}

/// BLAKE3 hex digest for app-level content hashing (distinct from BatPak event hashes).
pub fn blake3_hash_hex(input: &str) -> String {
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// BLAKE3 hex digest of raw bytes.
pub fn blake3_bytes_hex(input: &[u8]) -> String {
    blake3::hash(input).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_is_deterministic() {
        let hash = blake3_bytes_hex(b"hello");
        let a = source_id_from_hash(&hash).expect("source id");
        let b = source_id_from_hash(&hash).expect("source id");
        assert_eq!(a, b);
        assert!(a.as_str().starts_with("src_"));
    }

    #[test]
    fn claim_id_is_deterministic() {
        let src = SourceId::try_from("src_abc123def456").expect("source id");
        let a = claim_id_from_parts(&src, 4, "deploys happen on tuesday");
        let b = claim_id_from_parts(&src, 4, "deploys happen on tuesday");
        assert_eq!(a, b);
    }

    #[test]
    fn conflict_id_is_commutative() {
        let a = ClaimId::try_from("claim_aaaaaaaaaaaa").expect("claim a");
        let b = ClaimId::try_from("claim_bbbbbbbbbbbb").expect("claim b");
        assert_eq!(conflict_id_from_pair(&a, &b), conflict_id_from_pair(&b, &a));
    }

    #[test]
    fn source_id_from_hash_rejects_short_input() {
        // Fewer than SOURCE_ID_HASH_LEN (12) hex chars would silently truncate to
        // an ambiguous id, so it must error instead.
        let err = source_id_from_hash("abc").expect_err("short hash must error");
        assert_eq!(err, IdParseError::HashTooShort { min: 12 });
    }

    #[test]
    fn source_id_from_hash_accepts_exact_minimum_length() {
        let id = source_id_from_hash("0123456789ab").expect("exactly 12 chars");
        assert_eq!(id.as_str(), "src_0123456789ab");
    }

    #[test]
    fn branded_id_try_from_rejects_bad_prefix_and_empty() {
        assert_eq!(
            ClaimId::try_from("src_xyz"),
            Err(IdParseError::BadPrefix {
                expected: "claim_".to_string()
            })
        );
        assert_eq!(SourceId::try_from(""), Err(IdParseError::Empty));
    }

    #[test]
    fn branded_id_from_str_matches_try_from() {
        use std::str::FromStr;
        let via_from_str = ClaimId::from_str("claim_abc").expect("from_str");
        let via_try_from = ClaimId::try_from("claim_abc").expect("try_from");
        assert_eq!(via_from_str, via_try_from);
        assert_eq!(via_from_str.as_str(), "claim_abc");
        assert_eq!(via_from_str.as_ref(), "claim_abc");
        assert_eq!(via_from_str.to_string(), "claim_abc");
    }

    #[test]
    fn workspace_id_validates_and_builds_scope() {
        let ws = WorkspaceId::new("demo").expect("workspace");
        assert_eq!(ws.scope(), "workspace:demo");
        assert_eq!(ws.as_str(), "demo");
        assert_eq!(ws.as_ref(), "demo");
        assert_eq!(ws.to_string(), "demo");
        assert_eq!(
            WorkspaceId::try_from("bad/ws"),
            Err(IdParseError::InvalidWorkspace)
        );
    }
}
