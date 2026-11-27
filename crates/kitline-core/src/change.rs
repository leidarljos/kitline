use crate::error::{Error, Result};

/// jj reverse-hex alphabet: `z` is nibble 0 and `k` is nibble 15.
const REVERSE_HEX: &[u8; 16] = b"zyxwvutsrqponmlk";

/// 128-bit change identity. Display is 32 characters of jj reverse hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChangeId([u8; 16]);

impl ChangeId {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.len() != 32 {
            return Err(Error::Id(format!(
                "change id must be 32 reverse-hex chars: {raw}"
            )));
        }
        let mut out = [0u8; 16];
        let bytes = raw.as_bytes();
        for i in 0..16 {
            let hi = nibble(bytes[i * 2])?;
            let lo = nibble(bytes[i * 2 + 1])?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

fn nibble(b: u8) -> Result<u8> {
    let b = b.to_ascii_lowercase();
    match b {
        b'k'..=b'z' => Ok(b'z' - b),
        _ => Err(Error::Id(format!(
            "change id digit is outside k-z: {}",
            b as char
        ))),
    }
}

impl std::fmt::Display for ChangeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::with_capacity(32);
        for byte in self.0 {
            s.push(REVERSE_HEX[(byte >> 4) as usize] as char);
            s.push(REVERSE_HEX[(byte & 0xf) as usize] as char);
        }
        f.write_str(&s)
    }
}

/// SHA-256 of a revision's canonical series bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RevId([u8; 32]);

impl RevId {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let bytes = decode_hex(raw, 32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Self(out))
    }
}

impl std::fmt::Display for RevId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&encode_hex(&self.0))
    }
}

/// A git object id. SHA-1 is 40 hex characters and SHA-256 is 64.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.len() != 40 && raw.len() != 64 {
            return Err(Error::Id(format!("object id length {}", raw.len())));
        }
        if !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Id(format!("object id is not hex: {raw}")));
        }
        Ok(Self(raw.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn decode_hex(raw: &str, expect: usize) -> Result<Vec<u8>> {
    if raw.len() != expect * 2 {
        return Err(Error::Id(format!(
            "hex length {} is not {}",
            raw.len(),
            expect * 2
        )));
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(expect);
    for i in 0..expect {
        let hi = hex_val(bytes[i * 2])?;
        let lo = hex_val(bytes[i * 2 + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(Error::Id(format!("not hex: {}", b as char))),
    }
}
