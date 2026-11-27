use std::fs::File;
use std::io::Read;

use crate::change::ChangeId;
use crate::change::ObjectId;
use crate::error::{Error, Result};
use crate::gitcmd::git_stdin;

/// `mint` drew a new id, or `stamp` found one already in the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamped {
    pub id: ChangeId,
    /// Message bytes. Unchanged when a `Kitline-Change` trailer was already there.
    pub message: String,
    pub added: bool,
}

/// 128 random bits from `/dev/urandom`.
pub fn mint() -> ChangeId {
    let mut bytes = [0u8; 16];
    let mut file = File::open("/dev/urandom").expect("/dev/urandom");
    file.read_exact(&mut bytes).expect("/dev/urandom");
    ChangeId::from_bytes(bytes)
}

/// Read a `Kitline-Change` trailer from a commit message.
pub fn read(text: &str) -> Result<Option<ChangeId>> {
    message_change_id(text)
}

/// Read a raw commit object. A jj `change-id` header and a `Kitline-Change`
/// trailer must name the same change. Prose is not sniffed here.
pub fn read_commit(object: &str) -> Result<Option<ChangeId>> {
    let Some((headers, message)) = as_commit(object) else {
        return Err(Error::Trailer("not a raw commit object".into()));
    };
    let header = header_change_id(headers)?;
    let trailer = message_change_id(message)?;
    match (header, trailer) {
        (Some(header), Some(trailer)) if header != trailer => Err(Error::Disagree {
            header: header.to_string(),
            trailer: trailer.to_string(),
        }),
        (Some(header), _) => Ok(Some(header)),
        (None, trailer) => Ok(trailer),
    }
}

/// Append `Kitline-Change` when the message has none. An existing trailer
/// is left byte for byte, and its id is the one returned.
///
/// Placement is `git interpret-trailers`: beside the trailer block, before
/// a `---` divider. Bytes outside the inserted line stay as they were.
/// A message with no final LF gets the blank line the divider path inserts.
/// A message that already ends in LF is not rewritten first.
///
/// The id returned is the one read back from the message git wrote.
pub fn stamp(message: &str, id: &ChangeId) -> Result<Stamped> {
    if let Some(existing) = message_change_id(message)? {
        return Ok(Stamped {
            id: existing,
            message: message.to_string(),
            added: false,
        });
    }
    let trailer = format!("Kitline-Change: {id}");
    let padded;
    let input = if message.ends_with('\n') || has_divider(message) {
        message
    } else {
        padded = format!("{message}\n");
        padded.as_str()
    };
    let out = git_stdin(
        None,
        &[
            "-c",
            "trailer.separators=:",
            "interpret-trailers",
            "--trailer",
            &trailer,
        ],
        Some(input.as_bytes()),
    )?;
    let message = String::from_utf8(out)
        .map_err(|_| Error::Trailer("stamped message is not utf-8".into()))?;
    let written = message_change_id(&message)?;
    match written {
        Some(got) if got == *id => Ok(Stamped {
            id: got,
            message,
            added: true,
        }),
        Some(got) => Err(Error::Disagree {
            header: id.to_string(),
            trailer: got.to_string(),
        }),
        None => Err(Error::Trailer(
            "stamped message has no Kitline-Change".into(),
        )),
    }
}

/// Restamp `message` with the ChangeId of the commit it replaces.
///
/// `previous` is a raw commit or a message. No id in `message`: stamp the
/// previous id. The same id: return `message` unchanged. A different id
/// is refused. `git commit --amend -m` throws the old message away, so the
/// caller passes that commit in here.
pub fn carry(previous: &str, message: &str) -> Result<String> {
    let old = if as_commit(previous).is_some() {
        read_commit(previous)?
    } else {
        read(previous)?
    };
    let Some(old) = old else {
        return Err(Error::Trailer(
            "commit being replaced has no Kitline-Change".into(),
        ));
    };
    match message_change_id(message)? {
        None => {
            let stamped = stamp(message, &old)?;
            if stamped.id != old {
                return Err(Error::Disagree {
                    header: old.to_string(),
                    trailer: stamped.id.to_string(),
                });
            }
            Ok(stamped.message)
        }
        Some(found) if found == old => Ok(message.to_string()),
        Some(found) => Err(Error::Disagree {
            header: old.to_string(),
            trailer: found.to_string(),
        }),
    }
}

/// A newline-terminated `---` line. Git treats the following byte as the
/// divider only when it is whitespace, which includes the line's own LF.
/// A final `---` with no LF is not a divider, so the caller adds the LF.
fn has_divider(message: &str) -> bool {
    let Some((head, _)) = message.rsplit_once('\n') else {
        return false;
    };
    head.lines().any(is_divider_line)
}

fn is_divider_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("---") else {
        return false;
    };
    rest.is_empty() || rest.as_bytes()[0].is_ascii_whitespace()
}

fn message_change_id(message: &str) -> Result<Option<ChangeId>> {
    let block = kitline_ids(&trailer_lines(message)?)?;
    let earlier = ids_ending_earlier_paragraphs(message)?;
    match (block, earlier) {
        (Some(block), Some(earlier)) if block != earlier => Err(Error::Disagree {
            header: earlier.to_string(),
            trailer: block.to_string(),
        }),
        (Some(block), _) => Ok(Some(block)),
        (None, earlier) => Ok(earlier),
    }
}

/// `rebase -i` squash leaves `Kitline-Change:` at column 0 as the last line
/// of a paragraph, then a blank line and the next subject. The trailer block
/// is only that subject, so `--parse` misses the id. An indented quote is
/// not an id.
fn ids_ending_earlier_paragraphs(message: &str) -> Result<Option<ChangeId>> {
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for raw in message.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(line);
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    if paragraphs.len() < 2 {
        return Ok(None);
    }
    let mut found: Option<ChangeId> = None;
    for para in &paragraphs[..paragraphs.len() - 1] {
        let last = para[para.len() - 1];
        if last.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = last.split_once(':') else {
            continue;
        };
        if !key.eq_ignore_ascii_case("Kitline-Change") {
            continue;
        }
        let id = ChangeId::parse(value)
            .map_err(|_| Error::Trailer(format!("Kitline-Change is not a change id: {value}")))?;
        if let Some(prev) = found {
            if prev != id {
                return Err(Error::Disagree {
                    header: prev.to_string(),
                    trailer: id.to_string(),
                });
            }
        }
        found = Some(id);
    }
    Ok(found)
}

fn kitline_ids(lines: &[(String, String)]) -> Result<Option<ChangeId>> {
    let mut found: Option<ChangeId> = None;
    for (key, value) in lines {
        if !key.eq_ignore_ascii_case("Kitline-Change") {
            continue;
        }
        let id = ChangeId::parse(value)
            .map_err(|_| Error::Trailer(format!("Kitline-Change is not a change id: {value}")))?;
        if let Some(prev) = found {
            if prev != id {
                return Err(Error::Disagree {
                    header: prev.to_string(),
                    trailer: id.to_string(),
                });
            }
        }
        found = Some(id);
    }
    Ok(found)
}

/// Git's own trailer split. `git interpret-trailers --parse` is the rule.
fn trailer_lines(message: &str) -> Result<Vec<(String, String)>> {
    let out = git_stdin(
        None,
        &[
            "-c",
            "trailer.separators=:",
            "interpret-trailers",
            "--parse",
        ],
        Some(message.as_bytes()),
    )?;
    let text = String::from_utf8_lossy(&out);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(Error::Trailer(format!("unparsed trailer line: {line}")));
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        lines.push((key.to_string(), value.to_string()));
    }
    Ok(lines)
}

fn header_change_id(headers: &str) -> Result<Option<ChangeId>> {
    let mut found: Option<ChangeId> = None;
    for line in headers.lines() {
        let Some(rest) = line.strip_prefix("change-id ") else {
            continue;
        };
        let id = ChangeId::parse(rest.trim())
            .map_err(|_| Error::Trailer(format!("bad change-id header: {rest}")))?;
        if let Some(prev) = found {
            if prev != id {
                return Err(Error::Trailer("repeated change-id headers disagree".into()));
            }
        }
        found = Some(id);
    }
    Ok(found)
}

/// A raw commit starts with `tree <oid>` and has an `author` header.
fn as_commit(text: &str) -> Option<(&str, &str)> {
    let (headers, message) = text.split_once("\n\n")?;
    let mut first = headers.lines().next()?.split(' ');
    if first.next() != Some("tree") {
        return None;
    }
    let oid = first.next()?;
    if first.next().is_some() || ObjectId::parse(oid).is_err() {
        return None;
    }
    if !headers.lines().any(|line| line.starts_with("author ")) {
        return None;
    }
    Some((headers, message))
}
