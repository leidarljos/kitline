use crate::change::ObjectId;
use crate::error::{Error, Result};

/// Forge review handle. It moves when the review is recreated, and it is
/// not a commit. Text is `review/` plus a token.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReviewHandle(String);

/// Commit the forge currently has under review. It moves on every push.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReviewedHead(ObjectId);

/// Commit recorded as landed. It moves when the land commit is rewritten.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LandedSha(ObjectId);

/// Forge patch-set id. It moves when a new revision of the review is pushed.
/// Text is `set/` plus a token.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SetId(String);

impl ReviewHandle {
    pub fn parse(raw: &str) -> Result<Self> {
        parse_prefixed("review/", raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ReviewedHead {
    pub fn parse(raw: &str) -> Result<Self> {
        Ok(Self(ObjectId::parse(raw)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl LandedSha {
    pub fn parse(raw: &str) -> Result<Self> {
        Ok(Self(ObjectId::parse(raw)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl SetId {
    pub fn parse(raw: &str) -> Result<Self> {
        parse_prefixed("set/", raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_prefixed(prefix: &str, raw: &str) -> Result<String> {
    let raw = raw.trim();
    let Some(token) = raw.strip_prefix(prefix) else {
        return Err(Error::Id(format!("missing {prefix}: {raw}")));
    };
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(Error::Id(format!("bad token: {raw}")));
    }
    Ok(raw.to_string())
}

macro_rules! display_str {
    ($ty:ty) => {
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

display_str!(ReviewHandle);
display_str!(ReviewedHead);
display_str!(LandedSha);
display_str!(SetId);
