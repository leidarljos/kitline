use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use deedar::{is_write_once_addr, Body, Client, CreateRequest, DeedId, Kind};
use serde::{Deserialize, Serialize};

use crate::change::{encode_hex, ObjectId, RevId};
use crate::error::{Error, Result};
use crate::gitcmd::{git, git_text, TempDir};
use crate::rev::{resolve, revision, Revision};
use crate::trailer::read_commit;

const FILES: [&str; 5] = [
    "MSG.txt",
    "SHA256SUMS",
    "kit.bundle",
    "manifest.toml",
    "series.mbox",
];

/// A kit directory and the set deed that records its members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub deed_id: DeedId,
    pub revision: Revision,
    pub tip: ObjectId,
}

/// `base..tip` written as a deedar set deed.
///
/// Files in `out_dir`: `manifest.toml`, `series.mbox`, `kit.bundle`,
/// `MSG.txt`, and `SHA256SUMS`. `supersedes` is the previous kit deed.
pub fn kit_make(
    repo: &Path,
    range: &str,
    out_dir: &Path,
    store: &Path,
    supersedes: Option<&DeedId>,
) -> Result<Made> {
    let (base_spec, tip_spec) = split_range(range)?;
    let repo = abs(repo)?;
    let base = resolve(&repo, base_spec)?;
    let tip = resolve(&repo, tip_spec)?;
    let revision = revision(&repo, base.as_str(), tip.as_str())?;
    let listed = git(
        Some(&repo),
        &["rev-list", &format!("{}..{}", base.as_str(), tip.as_str())],
    )?;
    if listed.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::Kit("empty series".into()));
    }
    require_change_ids(&repo, base.as_str(), tip.as_str())?;
    let deed_id = DeedId::parse(&format!("deed-set-kit-{}-{}", revision.rev, base.as_str()))
        .map_err(|e| Error::Deed(e.to_string()))?;

    std::fs::create_dir_all(out_dir).map_err(|e| Error::Kit(e.to_string()))?;
    let out_dir = abs(out_dir)?;
    let bundle = out_dir.join("kit.bundle");
    write_bundle(&repo, &base, &tip, &bundle)?;
    let mbox = format_series(&repo, base.as_str(), tip.as_str())?;
    write_bytes(&out_dir.join("series.mbox"), &mbox)?;
    let manifest = Manifest {
        version: 1,
        base: base.as_str().to_string(),
        tip: tip.as_str().to_string(),
        rev: revision.rev.to_string(),
        deed: deed_id.to_string(),
    };
    let manifest_text = toml::to_string(&manifest).map_err(|e| Error::Kit(e.to_string()))?;
    let manifest_text = if manifest_text.ends_with('\n') {
        manifest_text
    } else {
        format!("{manifest_text}\n")
    };
    write_bytes(&out_dir.join("manifest.toml"), manifest_text.as_bytes())?;
    let msg = format!(
        "Kitline kit\nbase {}\ntip {}\nrev {}\n",
        base.as_str(),
        tip.as_str(),
        revision.rev
    );
    write_bytes(&out_dir.join("MSG.txt"), msg.as_bytes())?;
    write_sums(&out_dir)?;

    let mut members = Vec::new();
    for name in FILES {
        members.push(abs(&out_dir.join(name))?);
    }
    std::fs::create_dir_all(store).map_err(|e| Error::Kit(e.to_string()))?;
    let store = abs(store)?;
    let url = format!("file://{}", store.display());
    let mut client = Client::open(&url).map_err(|e| Error::Deed(e.to_string()))?;
    let (deed, _) = client
        .create(CreateRequest {
            id: Some(deed_id.clone()),
            name: format!("kit {}", revision.rev),
            sources: Vec::new(),
            agent_id: "kitline".into(),
            activity_id: None,
            // Grant::Path is stored on the deed and compared field for field.
            // The output directory is not the kit identity (rev + base), so
            // a second directory must not freeze an equal series.
            grants: Vec::new(),
            body: Body::Set {
                title: format!("kit {}", revision.rev),
                members,
            },
            supersedes: supersedes.cloned(),
        })
        .map_err(|e| Error::Deed(e.to_string()))?;
    if deed.id != deed_id {
        return Err(Error::Kit(format!(
            "store minted {} instead of {deed_id}",
            deed.id
        )));
    }
    Ok(Made {
        deed_id,
        revision,
        tip,
    })
}

/// Check member addresses, the bundle, the mbox, and the rev.
/// `git am` of `series.mbox` onto the prerequisite must reproduce the
/// bundle tip's tree and rev.
pub fn kit_verify(repo: &Path, kit_dir: &Path, store: &Path) -> Result<Revision> {
    let repo = abs(repo)?;
    let kit_dir = abs(kit_dir)?;
    let store = abs(store)?;
    check_sums(&kit_dir)?;
    let manifest: Manifest = {
        let text = std::fs::read_to_string(kit_dir.join("manifest.toml"))
            .map_err(|e| Error::Kit(e.to_string()))?;
        toml::from_str(&text).map_err(|e| Error::Kit(e.to_string()))?
    };
    if manifest.version != 1 {
        return Err(Error::Kit(format!("manifest version {}", manifest.version)));
    }
    let deed_id = DeedId::parse(&manifest.deed).map_err(|e| Error::Deed(e.to_string()))?;
    let url = format!("file://{}", store.display());
    let mut client = Client::open(&url).map_err(|e| Error::Deed(e.to_string()))?;
    let deed = client
        .get(&deed_id)
        .map_err(|e| Error::Deed(e.to_string()))?;
    if deed.kind != Kind::Set {
        return Err(Error::Kit(format!("deed {} is not a set", deed.id)));
    }
    let Body::Set { members, .. } = &deed.body else {
        return Err(Error::Kit("deed body is not a set".into()));
    };
    let mut from_deed = BTreeSet::new();
    for member in members {
        let Some(hex) = member
            .to_str()
            .filter(|text| is_write_once_addr(text))
            .and_then(|text| text.strip_prefix("sha256:"))
        else {
            return Err(Error::Kit(format!(
                "member {} is not a content address",
                member.display()
            )));
        };
        let stored = std::fs::read(store.join("bytes").join(hex))
            .map_err(|e| Error::Kit(format!("bytes/{hex}: {e}")))?;
        let got = sha256_hex(&stored);
        if got != hex {
            return Err(Error::Kit(format!("bytes/{hex} hashes to {got}")));
        }
        from_deed.insert(hex.to_string());
    }
    let mut from_files = BTreeSet::new();
    for name in FILES {
        from_files.insert(file_sha(&kit_dir.join(name))?);
    }
    if from_files != from_deed {
        return Err(Error::Kit("deed members do not match the kit files".into()));
    }

    let mbox = std::fs::read(kit_dir.join("series.mbox")).map_err(|e| Error::Kit(e.to_string()))?;
    let bundle = kit_dir.join("kit.bundle");
    let tmp = TempDir::new()?;
    let clone = tmp.path().join("repo");
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--local",
            repo.to_str()
                .ok_or_else(|| Error::Kit("repo path is not utf-8".into()))?,
            clone
                .to_str()
                .ok_or_else(|| Error::Kit("clone path is not utf-8".into()))?,
        ],
    )?;
    let bundle_arg = bundle
        .to_str()
        .ok_or_else(|| Error::Kit("bundle path is not utf-8".into()))?;
    git(Some(&clone), &["bundle", "verify", bundle_arg])?;
    git(
        Some(&clone),
        &[
            "fetch",
            "--quiet",
            bundle_arg,
            "refs/heads/kit-tip:refs/kitline/tip",
        ],
    )?;
    let fetched = resolve(&clone, "refs/kitline/tip")?;
    if fetched.as_str() != manifest.tip {
        return Err(Error::Kit(format!(
            "bundle tip {} does not match manifest {}",
            fetched.as_str(),
            manifest.tip
        )));
    }
    let regenerated = format_series(&clone, &manifest.base, "refs/kitline/tip")?;
    if regenerated != mbox {
        return Err(Error::Kit(
            "series.mbox does not regenerate from the bundle".into(),
        ));
    }
    applied_matches(
        &clone,
        &kit_dir.join("series.mbox"),
        &manifest.base,
        "refs/kitline/tip",
    )?;
    let checked = revision(&clone, &manifest.base, "refs/kitline/tip")?;
    if checked.rev.to_string() != manifest.rev {
        return Err(Error::Kit(format!(
            "bundle rev {} does not match manifest {}",
            checked.rev, manifest.rev
        )));
    }
    if checked.base.as_str() != manifest.base {
        return Err(Error::Kit("bundle base does not match the manifest".into()));
    }
    let _ = RevId::parse(&manifest.rev)?;
    Ok(checked)
}

fn write_bundle(repo: &Path, base: &ObjectId, tip: &ObjectId, dest: &Path) -> Result<()> {
    let scratch = TempDir::new()?;
    let clone = scratch.path().join("repo");
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--local",
            repo.to_str()
                .ok_or_else(|| Error::Kit("repo path is not utf-8".into()))?,
            clone
                .to_str()
                .ok_or_else(|| Error::Kit("clone path is not utf-8".into()))?,
        ],
    )?;
    git(
        Some(&clone),
        &["update-ref", "refs/heads/kit-tip", tip.as_str()],
    )?;
    refuse_symlink(dest)?;
    let tmp = scratch_name(dest)?;
    let tmp_arg = tmp
        .to_str()
        .ok_or_else(|| Error::Kit("bundle path is not utf-8".into()))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| Error::Kit(format!("{}: {e}", tmp.display())))?;
    let exclude = format!("^{}", base.as_str());
    if let Err(e) = git(
        Some(&clone),
        &["bundle", "create", tmp_arg, "refs/heads/kit-tip", &exclude],
    ) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    publish(&tmp, dest)
}

fn format_series(repo: &Path, base: &str, tip: &str) -> Result<Vec<u8>> {
    if base.starts_with('-') || tip.starts_with('-') {
        return Err(Error::Kit("revision must not be an option".into()));
    }
    // Resolve before the scratch clone. Clone copies objects, not every ref,
    // so `refs/kitline/tip` would not exist there.
    let base_id = resolve(repo, base)?;
    let tip_id = resolve(repo, tip)?;
    // format-patch in a scratch clone so the source repo's format.headers
    // (and any other mail config clone does not copy) cannot change the mbox.
    // diff.renames and format.headers are pinned on the command line as well.
    let scratch = TempDir::new()?;
    let clone = scratch.path().join("repo");
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--local",
            repo.to_str()
                .ok_or_else(|| Error::Kit("repo path is not utf-8".into()))?,
            clone
                .to_str()
                .ok_or_else(|| Error::Kit("clone path is not utf-8".into()))?,
        ],
    )?;
    let range = format!("{}..{}", base_id.as_str(), tip_id.as_str());
    git(
        Some(&clone),
        &[
            "-c",
            "format.numbered=auto",
            "-c",
            "format.signOff=false",
            "-c",
            "format.signature=",
            "-c",
            "format.coverLetter=false",
            "-c",
            "format.useAutoBase=false",
            "-c",
            "format.subjectPrefix=PATCH",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "-c",
            "diff.renames=true",
            "-c",
            "format.headers=",
            "-c",
            "core.abbrev=40",
            "-c",
            "i18n.logOutputEncoding=utf-8",
            "format-patch",
            "--stdout",
            "--no-signature",
            "--end-of-options",
            &range,
        ],
    )
}

/// Apply `series.mbox` with `git am` onto `base` and require the same
/// tree and rev as `tip`. A `---` line in a message makes am drop the
/// bytes after it, so the rev no longer matches.
fn applied_matches(repo: &Path, mbox: &Path, base: &str, tip: &str) -> Result<()> {
    let scratch = TempDir::new()?;
    let wt = scratch.path().join("wt");
    let wt_arg = wt
        .to_str()
        .ok_or_else(|| Error::Kit("worktree path is not utf-8".into()))?;
    git(
        Some(repo),
        &["worktree", "add", "--detach", "--quiet", wt_arg, base],
    )?;
    let mbox_arg = mbox
        .to_str()
        .ok_or_else(|| Error::Kit("mbox path is not utf-8".into()))?;
    git(
        Some(&wt),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Kitline",
            "-c",
            "user.email=kitline@localhost",
            "am",
            "--quiet",
            mbox_arg,
        ],
    )?;
    let applied = revision(&wt, base, "HEAD")?;
    let bundled = revision(repo, base, tip)?;
    let applied_tree = git_text(Some(&wt), &["rev-parse", "--verify", "HEAD^{tree}"])?;
    let tree_spec = format!("{tip}^{{tree}}");
    let bundled_tree = git_text(Some(repo), &["rev-parse", "--verify", &tree_spec])?;
    let _ = git(Some(repo), &["worktree", "remove", "--force", wt_arg]);
    if applied.rev != bundled.rev {
        return Err(Error::Kit("git am rev does not match the bundle".into()));
    }
    if applied_tree != bundled_tree {
        return Err(Error::Kit("git am tree does not match the bundle".into()));
    }
    Ok(())
}

fn write_sums(dir: &Path) -> Result<()> {
    let mut text = String::new();
    for name in ["MSG.txt", "kit.bundle", "manifest.toml", "series.mbox"] {
        let hex = file_sha(&dir.join(name))?;
        text.push_str(&hex);
        text.push_str("  ");
        text.push_str(name);
        text.push('\n');
    }
    write_bytes(&dir.join("SHA256SUMS"), text.as_bytes())
}

fn check_sums(dir: &Path) -> Result<()> {
    let text =
        std::fs::read_to_string(dir.join("SHA256SUMS")).map_err(|e| Error::Kit(e.to_string()))?;
    let mut n = 0u32;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((hex, name)) = line.split_once("  ") else {
            return Err(Error::Kit(format!("bad SHA256SUMS line: {line}")));
        };
        if name == "SHA256SUMS" {
            return Err(Error::Kit("SHA256SUMS lists itself".into()));
        }
        let got = file_sha(&kit_child(dir, name)?)?;
        if got != hex {
            return Err(Error::Kit(format!("{name} checksum mismatch")));
        }
        n += 1;
    }
    if n != 4 {
        return Err(Error::Kit(format!("SHA256SUMS has {n} entries")));
    }
    Ok(())
}

/// A new file in `path`'s directory, then rename onto `path`.
/// A pre-existing symlink is refused. A hard link is replaced by name,
/// so the other link keeps its bytes.
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    refuse_symlink(path)?;
    let tmp = scratch_name(path)?;
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| Error::Kit(format!("{}: {e}", tmp.display())))?;
        std::io::Write::write_all(&mut file, bytes)
            .map_err(|e| Error::Kit(format!("{}: {e}", tmp.display())))?;
    }
    publish(&tmp, path)
}

fn publish(tmp: &Path, dest: &Path) -> Result<()> {
    if let Err(e) = std::fs::rename(tmp, dest) {
        let _ = std::fs::remove_file(tmp);
        return Err(Error::Kit(format!("{}: {e}", dest.display())));
    }
    Ok(())
}

fn refuse_symlink(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(Error::Kit(format!("{} is a symlink", path.display())))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::Kit(e.to_string())),
        _ => Ok(()),
    }
}

fn scratch_name(path: &Path) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| Error::Kit(format!("{} has no file name", path.display())))?;
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut tmp = std::ffi::OsString::from(".");
    tmp.push(name);
    tmp.push(format!(".{}.{n}.tmp", std::process::id()));
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(parent.join(tmp))
}

/// Names from SHA256SUMS stay inside `dir`. `/`, `..`, and absolute paths
/// are refused before any open.
fn kit_child(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    let inside = !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if !inside {
        return Err(Error::Kit(format!(
            "SHA256SUMS name is outside the kit: {name}"
        )));
    }
    Ok(dir.join(name))
}

fn file_sha(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| Error::Kit(format!("{}: {e}", path.display())))?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_hex(&hasher.finalize())
}

/// A series commit with no ChangeId is refused. Never mint one here.
fn require_change_ids(repo: &Path, base: &str, tip: &str) -> Result<()> {
    let listed = git_text(
        Some(repo),
        &["rev-list", "--reverse", &format!("{base}..{tip}")],
    )?;
    if listed.is_empty() {
        return Ok(());
    }
    for sha in listed.split_whitespace() {
        let raw = git(Some(repo), &["cat-file", "commit", sha])?;
        let text =
            String::from_utf8(raw).map_err(|_| Error::Kit(format!("commit {sha} is not utf-8")))?;
        if read_commit(&text)?.is_some() {
            continue;
        }
        let subject = text
            .split_once("\n\n")
            .and_then(|(_, msg)| msg.lines().next())
            .unwrap_or("");
        let subject = subject.trim_end_matches('\r');
        return Err(Error::Kit(format!(
            "commit {sha} ({subject}) has no ChangeId"
        )));
    }
    Ok(())
}

fn split_range(range: &str) -> Result<(&str, &str)> {
    if range.contains("...") {
        return Err(Error::Kit(format!(
            "three-dot range is not a series: {range}"
        )));
    }
    let Some((base, tip)) = range.split_once("..") else {
        return Err(Error::Kit(format!("range wants base..tip: {range}")));
    };
    if base.is_empty() || tip.is_empty() {
        return Err(Error::Kit(format!("range wants base..tip: {range}")));
    }
    Ok((base, tip))
}

fn abs(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|e| Error::Kit(format!("{}: {e}", path.display())))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    version: u32,
    base: String,
    tip: String,
    rev: String,
    deed: String,
}
