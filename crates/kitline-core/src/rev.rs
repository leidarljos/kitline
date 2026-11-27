use std::path::Path;

use sha2::{Digest, Sha256};

use crate::change::{ObjectId, RevId};
use crate::error::{Error, Result};
use crate::gitcmd::{git, git_stdin, git_text};

/// A revision is the series content and the base it sits on.
/// The base is not an input to [`RevId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Revision {
    pub rev: RevId,
    pub base: ObjectId,
}

/// Same series content on the same base.
pub fn identical(a: &Revision, b: &Revision) -> bool {
    a.rev == b.rev && a.base == b.base
}

/// Same series content on a different base.
pub fn patch_equal(a: &Revision, b: &Revision) -> bool {
    a.rev == b.rev && a.base != b.base
}

/// Hint only. Equal trees do not mean the revisions match.
pub fn tree_equal(a: &ObjectId, b: &ObjectId) -> bool {
    a == b
}

/// `RevId` of `base..tip`. `base` is recorded and not hashed.
pub fn revision(repo: &Path, base: &str, tip: &str) -> Result<Revision> {
    let base_id = resolve(repo, base)?;
    let tip_id = resolve(repo, tip)?;
    let rev = rev_of(repo, base_id.as_str(), tip_id.as_str())?;
    Ok(Revision { rev, base: base_id })
}

pub(crate) fn resolve(repo: &Path, spec: &str) -> Result<ObjectId> {
    let spec = format!("{spec}^{{commit}}");
    let text = git_text(Some(repo), &["rev-parse", "--verify", &spec])?;
    ObjectId::parse(&text)
}

fn rev_of(repo: &Path, base: &str, tip: &str) -> Result<RevId> {
    let range = format!("{base}..{tip}");
    let listed = git_text(Some(repo), &["rev-list", "--reverse", &range])?;
    let mut hasher = Sha256::new();
    hasher.update(b"kitline-revid-v1\n");
    if !listed.is_empty() {
        for sha in listed.split_whitespace() {
            feed_commit(&mut hasher, repo, sha)?;
        }
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(RevId::from_bytes(bytes))
}

/// Per commit, in series order: 20-byte `git patch-id --verbatim`, then the
/// exact message bytes, then the author ident `Name <email>`. Lengths are
/// big-endian u32. The base commit is not included.
fn feed_commit(hasher: &mut Sha256, repo: &Path, sha: &str) -> Result<()> {
    let patch = patch_id(repo, sha)?;
    let raw = git(Some(repo), &["cat-file", "commit", sha])?;
    let (headers, message) = split_once(&raw, b"\n\n")
        .ok_or_else(|| Error::Git(format!("commit {sha} has no message separator")))?;
    let author = author_ident(headers)?;
    hasher.update(patch);
    feed_bytes(hasher, message)?;
    feed_bytes(hasher, &author)?;
    Ok(())
}

fn feed_bytes(hasher: &mut Sha256, bytes: &[u8]) -> Result<()> {
    let len =
        u32::try_from(bytes.len()).map_err(|_| Error::Git("commit field is too long".into()))?;
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
    Ok(())
}

fn patch_id(repo: &Path, sha: &str) -> Result<[u8; 20]> {
    let diff = git(
        Some(repo),
        &[
            "-c",
            "core.abbrev=40",
            "diff-tree",
            "--no-color",
            "--patch",
            "--binary",
            "--root",
            "-r",
            "--full-index",
            sha,
        ],
    )?;
    let out = git_stdin(None, &["patch-id", "--verbatim"], Some(&diff))?;
    let text = String::from_utf8_lossy(&out);
    let hex = text
        .split_whitespace()
        .next()
        .ok_or_else(|| Error::Git(format!("patch-id returned nothing for {sha}")))?;
    let bytes = decode_fixed(hex)?;
    Ok(bytes)
}

fn decode_fixed(hex: &str) -> Result<[u8; 20]> {
    if hex.len() != 40 {
        return Err(Error::Git(format!("patch-id {hex} is not 40 hex chars")));
    }
    let mut out = [0u8; 20];
    let raw = hex.as_bytes();
    for i in 0..20 {
        let hi = hex_val(raw[i * 2])?;
        let lo = hex_val(raw[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(Error::Git(format!("patch-id digit {}", b as char))),
    }
}

fn author_ident(headers: &[u8]) -> Result<Vec<u8>> {
    for line in headers.split(|b| *b == b'\n') {
        let Some(rest) = line.strip_prefix(b"author ") else {
            continue;
        };
        let mut parts = rest.rsplitn(3, |b| *b == b' ');
        let tz = parts.next().unwrap_or_default();
        let ts = parts.next().unwrap_or_default();
        let ident = parts.next().unwrap_or_default();
        if is_tz(tz) && is_unix(ts) && ident.last() == Some(&b'>') {
            return Ok(ident.to_vec());
        }
        return Err(Error::Git(format!(
            "author line is not an ident: {}",
            String::from_utf8_lossy(line)
        )));
    }
    Err(Error::Git("commit has no author".into()))
}

fn is_tz(tz: &[u8]) -> bool {
    tz.len() == 5 && (tz[0] == b'+' || tz[0] == b'-') && tz[1..].iter().all(u8::is_ascii_digit)
}

fn is_unix(ts: &[u8]) -> bool {
    !ts.is_empty() && ts.iter().all(u8::is_ascii_digit)
}

fn split_once<'a>(raw: &'a [u8], sep: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    raw.windows(sep.len())
        .position(|window| window == sep)
        .map(|i| (&raw[..i], &raw[i + sep.len()..]))
}
