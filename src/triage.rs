//! Triage phases 1, 2 and 6, plus a landing order.
//!
//! Phase 1 inventories every head. Phase 2 grades what is already
//! upstream with patch-id, cherry and merge-base. Phase 6 records
//! freshness. The plan lists KEEP, DROP, REBASE, the remaining heads, and
//! an unrelated head as UNSORTED. Heads that still need a decision are
//! grouped into stacks: a head whose tip contains another head is stacked
//! on it. ORDER lists parents first, which is the land sequence.
//! Rows use full ref names. Git commands take the commit sha.
//! `triage verify` re-reads that ref and re-runs each DROP check.
//! It rejects an ORDER that lists a parent after its child.

use crate::gitutil;
use clap::Parser;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Patch-id of a trunk commit against its first parent. `None` is a root
/// or an empty diff. Shared across heads so each trunk commit is diffed once.
type PatchCache = HashMap<String, Option<String>>;

const COLS: &[&str] = &[
    "branch",
    "sha",
    "base",
    "ahead",
    "behind",
    "author_old",
    "author_new",
    "grade",
    "check",
    "evidence",
    "applies",
    "conflicts",
    "dropped",
    "reason",
];

#[derive(Parser)]
#[command(name = "kitline", about = "Grade heads that are already upstream")]
struct TriageCli {
    /// Trunk head to grade against.
    #[arg(long, default_value = "main")]
    trunk: String,
    /// Remote used when the trunk is not a local head.
    #[arg(long, default_value = "origin")]
    remote: String,
    /// Org plan to write.
    #[arg(long, default_value = "plan.org")]
    out: PathBuf,
    /// Heads matching a glob stay in KEEP.
    #[arg(long = "keep")]
    keep: Vec<String>,
    /// When set, only heads matching one of these globs are inventoried.
    #[arg(long = "include")]
    include: Vec<String>,
    /// Heads matching a glob are left out of the plan. The trunk stays.
    #[arg(long = "exclude")]
    exclude: Vec<String>,
    /// Local path or URL. A local repo is cloned into a private bare store.
    repo: PathBuf,
}

struct Out {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct Scratch {
    path: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Head {
    name: Vec<u8>,
    sha: String,
}

struct Ctx<'a> {
    repo: &'a Path,
    trunk_ref: &'a [u8],
    trunk_sha: &'a str,
    trunk_tree: &'a str,
}

struct Cherry {
    upstream: bool,
    sha: String,
}

struct Merge {
    clean: bool,
    tree: String,
    conflicts: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Keep,
    Drop,
    Rebase,
    Rest,
    Unsorted,
}

impl Bucket {
    fn title(self) -> &'static str {
        match self {
            Self::Keep => "KEEP",
            Self::Drop => "DROP",
            Self::Rebase => "REBASE",
            Self::Rest => "REST",
            Self::Unsorted => "UNSORTED",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Drop => "drop",
            Self::Rebase => "rebase",
            Self::Rest => "rest",
            Self::Unsorted => "unsorted",
        }
    }
}

struct Row {
    branch: Vec<u8>,
    sha: String,
    base: String,
    ahead: u64,
    behind: u64,
    author_old: String,
    author_new: String,
    author_new_epoch: i64,
    grade: String,
    bucket: Bucket,
    check: String,
    evidence: String,
    applies: String,
    conflicts: String,
    dropped: String,
    reason: String,
}

struct DropRow {
    branch: Vec<u8>,
    grade: String,
    check: String,
}

pub fn run() -> i32 {
    let args = match utf8_args(1) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("triage: {err}");
            return 1;
        }
    };
    let rest: &[String] = if args.first().map(String::as_str) == Some("triage") {
        &args[1..]
    } else {
        &args
    };
    if rest.first().map(String::as_str) == Some("verify") {
        if rest.len() != 2 {
            eprintln!("usage: kitline triage verify <plan.org>");
            return 2;
        }
        return finish(verify(Path::new(&rest[1])));
    }
    let mut argv = vec!["kitline".to_string()];
    argv.extend(rest.iter().cloned());
    let cli = TriageCli::parse_from(argv);
    finish(plan(&cli))
}

fn utf8_args(skip: usize) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for arg in std::env::args_os().skip(skip) {
        match arg.to_str() {
            Some(text) => out.push(text.to_string()),
            None => return Err("argument is not utf-8".into()),
        }
    }
    Ok(out)
}

fn finish(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("triage: {err}");
            1
        }
    }
}

fn plan(cli: &TriageCli) -> Result<(), String> {
    let repo = locate(&cli.repo)?;
    require_full_history(&repo)?;
    let scratch = scratch_dir()?;
    let bare = scratch.path.join("bare.git");
    clone_bare(&repo, &bare)?;
    let trunk_ref = trunk_ref(&bare, &cli.trunk, &cli.remote)?;
    let trunk_sha = trunk_tip(&bare, &cli.trunk, &cli.remote)?;
    let trunk_tree = git_ok(
        &bare,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{trunk_sha}^{{tree}}"),
        ],
    )?;
    let heads = selected_heads(&bare, &trunk_ref, &cli.include, &cli.exclude)?;
    if !heads.iter().any(|h| h.name == trunk_ref) {
        return Err(format!("trunk {} is not a head", cli.trunk));
    }
    let snap = snapshot(&heads);
    let ctx = Ctx {
        repo: &bare,
        trunk_ref: &trunk_ref,
        trunk_sha: &trunk_sha,
        trunk_tree: &trunk_tree,
    };
    let mut rows = Vec::with_capacity(heads.len());
    let mut patches = PatchCache::new();
    for head in &heads {
        let short = short_of(&head.name);
        let kept = head.name == trunk_ref || cli.keep.iter().any(|pat| glob_match(pat, short));
        rows.push(classify(&ctx, head, kept, &mut patches)?);
    }
    let land = land_plan(&bare, &rows)?;
    let mut text = render(&repo, &cli.trunk, &cli.remote, &trunk_sha, &snap, &rows)?;
    text.push_str(&land.text);
    write_text(&cli.out, &text)?;
    for bucket in [
        Bucket::Keep,
        Bucket::Drop,
        Bucket::Rebase,
        Bucket::Rest,
        Bucket::Unsorted,
    ] {
        let n = rows.iter().filter(|r| r.bucket == bucket).count();
        println!("{} {n}", bucket.key());
    }
    println!("stacks {}", land.stacks);
    println!("order {}", land.order);
    for line in &land.lines {
        println!("{line}");
    }
    println!("wrote {}", cli.out.display());
    Ok(())
}

fn verify(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("bad plan: {e}"))?;
    let (repo, trunk, remote, drops) = parse_drops(&text)?;
    require_full_history(&repo)?;
    let scratch = scratch_dir()?;
    let bare = scratch.path.join("bare.git");
    clone_bare(&repo, &bare)?;
    let trunk_ref = trunk_ref(&bare, &trunk, &remote)?;
    let trunk_sha = trunk_tip(&bare, &trunk, &remote)?;
    let trunk_tree = git_ok(
        &bare,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{trunk_sha}^{{tree}}"),
        ],
    )?;
    let ctx = Ctx {
        repo: &bare,
        trunk_ref: &trunk_ref,
        trunk_sha: &trunk_sha,
        trunk_tree: &trunk_tree,
    };
    let mut failed = Vec::new();
    let mut patches = PatchCache::new();
    for row in &drops {
        if row.grade != row.check || row.check.is_empty() {
            failed.push(format!("drop does not hold: {}", escape_cell(&row.branch)));
            continue;
        }
        let full = head_ref(&row.branch);
        if !branch_exists(&bare, &full)? {
            failed.push(format!("drop does not hold: {}", escape_cell(&row.branch)));
            continue;
        }
        // Re-read the live tip of the full ref. The sha in the plan is only
        // the evidence from plan time.
        let live = live_sha(&bare, &full)?;
        let holds = check_holds(&ctx, &live, &row.check, &mut patches)?;
        if !holds {
            failed.push(format!("drop does not hold: {}", escape_cell(&row.branch)));
        }
    }
    order_holds(&bare, &text, &mut failed)?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("\n"))
    }
}

fn check_holds(
    ctx: &Ctx<'_>,
    sha: &str,
    check: &str,
    patches: &mut PatchCache,
) -> Result<bool, String> {
    let merge = merge_outcome(ctx.repo, ctx.trunk_sha, sha)?;
    let noop = merge.clean && merge.tree == ctx.trunk_tree;
    match check {
        "ancestor" => Ok(gitutil::is_ancestor(ctx.repo, sha, ctx.trunk_sha)),
        "patch-equal" => {
            let cherries = cherry(ctx.repo, ctx.trunk_sha, sha)?;
            let all = !cherries.is_empty() && cherries.iter().all(|c| c.upstream);
            Ok(all && noop)
        }
        "squash-equal" => {
            let Some(base) = merge_base(ctx.repo, ctx.trunk_sha, sha)? else {
                return Ok(false);
            };
            let hit = squash_hit(ctx.repo, &base, ctx.trunk_sha, sha, patches)?;
            Ok(hit.is_some() && noop)
        }
        "no-op" => Ok(noop),
        _ => Ok(false),
    }
}

fn classify(
    ctx: &Ctx<'_>,
    head: &Head,
    kept: bool,
    patches: &mut PatchCache,
) -> Result<Row, String> {
    let short = short_of(&head.name);
    if !name_ok(short) {
        return Err(format!(
            "branch name {} cannot be stored in the plan",
            escape_cell(short)
        ));
    }
    let (ahead, behind) = gitutil::ahead_behind(ctx.repo, ctx.trunk_sha, &head.sha)
        .ok_or_else(|| format!("ahead/behind for {}", escape_cell(&head.name)))?;
    let (author_old, author_new, author_new_epoch) =
        author_range(ctx.repo, ctx.trunk_sha, &head.sha)?;
    let Some(base) = merge_base(ctx.repo, ctx.trunk_sha, &head.sha)? else {
        return Ok(Row {
            branch: head.name.clone(),
            sha: head.sha.clone(),
            base: String::new(),
            ahead,
            behind,
            author_old,
            author_new,
            author_new_epoch,
            grade: "unrelated".to_string(),
            bucket: Bucket::Unsorted,
            check: String::new(),
            evidence: String::new(),
            applies: "no".to_string(),
            conflicts: String::new(),
            dropped: String::new(),
            reason: "unrelated".to_string(),
        });
    };
    let merge = merge_outcome(ctx.repo, ctx.trunk_sha, &head.sha)?;
    let applies = if merge.clean { "yes" } else { "no" };
    let conflicts = join_list(&merge.conflicts);
    let noop = merge.clean && merge.tree == ctx.trunk_tree;
    let decided = decide(ctx, head, kept, &base, &merge, noop, patches)?;

    Ok(Row {
        branch: head.name.clone(),
        sha: head.sha.clone(),
        base,
        ahead,
        behind,
        author_old,
        author_new,
        author_new_epoch,
        grade: decided.grade,
        bucket: decided.bucket,
        check: decided.check,
        evidence: decided.evidence,
        applies: applies.to_string(),
        conflicts,
        dropped: decided.dropped,
        reason: decided.reason.to_string(),
    })
}

struct Decision {
    grade: String,
    bucket: Bucket,
    check: String,
    evidence: String,
    dropped: String,
    reason: &'static str,
}

fn drop_of(grade: &str, evidence: String, reason: &'static str) -> Decision {
    Decision {
        grade: grade.to_string(),
        bucket: Bucket::Drop,
        check: grade.to_string(),
        evidence,
        dropped: String::new(),
        reason,
    }
}

fn plain(grade: &str, bucket: Bucket, reason: &'static str) -> Decision {
    Decision {
        grade: grade.to_string(),
        bucket,
        check: String::new(),
        evidence: String::new(),
        dropped: String::new(),
        reason,
    }
}

fn decide(
    ctx: &Ctx<'_>,
    head: &Head,
    kept: bool,
    base: &str,
    merge: &Merge,
    noop: bool,
    patches: &mut PatchCache,
) -> Result<Decision, String> {
    if kept {
        let reason = if head.name == ctx.trunk_ref {
            "trunk"
        } else {
            "keep"
        };
        return Ok(plain("keep", Bucket::Keep, reason));
    }
    if gitutil::is_ancestor(ctx.repo, &head.sha, ctx.trunk_sha) {
        return Ok(drop_of(
            "ancestor",
            format!("is-ancestor {} {}", head.sha, ctx.trunk_sha),
            "ancestor of trunk",
        ));
    }
    let cherries = cherry(ctx.repo, ctx.trunk_sha, &head.sha)?;
    let all_up = !cherries.is_empty() && cherries.iter().all(|c| c.upstream);
    let some_up = cherries.iter().any(|c| c.upstream);
    let some_new = cherries.iter().any(|c| !c.upstream);
    if all_up && noop {
        return Ok(drop_of(
            "patch-equal",
            format!(
                "cherry {}; merge-tree {}",
                sha_list(&cherries, true),
                merge.tree
            ),
            "patch-equal and no-op",
        ));
    }
    if all_up {
        return Ok(plain(
            "patch-equal",
            Bucket::Rest,
            "merged then reverted or modified upstream",
        ));
    }
    if let Some((pid, commit)) = squash_hit(ctx.repo, base, ctx.trunk_sha, &head.sha, patches)? {
        if noop {
            return Ok(drop_of(
                "squash-equal",
                format!("patch-id {pid} {commit}; merge-tree {}", merge.tree),
                "squash-equal and no-op",
            ));
        }
        return Ok(plain(
            "squash-equal",
            Bucket::Rest,
            "squash-equal without no-op",
        ));
    }
    if noop {
        return Ok(drop_of(
            "no-op",
            format!("merge-tree {}", merge.tree),
            "merge-tree equals trunk",
        ));
    }
    if some_up && some_new {
        return Ok(Decision {
            grade: "partial".to_string(),
            bucket: Bucket::Rebase,
            check: String::new(),
            evidence: String::new(),
            dropped: sha_list(&cherries, true),
            reason: "partial upstream",
        });
    }
    if !merge.clean {
        return Ok(plain("open", Bucket::Rebase, "does not apply"));
    }
    Ok(plain("open", Bucket::Rest, "applies"))
}

fn sha_list(cherries: &[Cherry], upstream: bool) -> String {
    cherries
        .iter()
        .filter(|c| c.upstream == upstream)
        .map(|c| c.sha.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn join_list(items: &[String]) -> String {
    let mut items = items.to_vec();
    items.sort();
    items.dedup();
    items.join(",")
}

fn render(
    repo: &Path,
    trunk: &str,
    remote: &str,
    trunk_sha: &str,
    snap: &str,
    rows: &[Row],
) -> Result<String, String> {
    let repo = repo
        .to_str()
        .ok_or_else(|| "repo path is not utf-8".to_string())?;
    if repo.contains(['\n', '\r']) || trunk.contains(['\n', '\r']) || remote.contains(['\n', '\r'])
    {
        return Err("repo path, trunk, or remote contains a newline".into());
    }
    let mut buf = String::new();
    buf.push_str("#+title: kitline triage\n");
    buf.push_str(&format!("#+repo: {}\n", escape_cell(repo.as_bytes())));
    buf.push_str(&format!("#+trunk: {}\n", escape_cell(trunk.as_bytes())));
    buf.push_str(&format!("#+remote: {}\n", escape_cell(remote.as_bytes())));
    buf.push_str(&format!("#+trunk_sha: {trunk_sha}\n"));
    buf.push_str(&format!("#+snapshot: {snap}\n"));
    buf.push_str(&format!("#+version: {}\n", env!("CARGO_PKG_VERSION")));
    for bucket in [
        Bucket::Keep,
        Bucket::Drop,
        Bucket::Rebase,
        Bucket::Rest,
        Bucket::Unsorted,
    ] {
        let n = rows.iter().filter(|r| r.bucket == bucket).count();
        buf.push_str(&format!("#+{}: {n}\n", bucket.key()));
    }
    buf.push('\n');
    for bucket in [
        Bucket::Keep,
        Bucket::Drop,
        Bucket::Rebase,
        Bucket::Rest,
        Bucket::Unsorted,
    ] {
        emit_bucket(&mut buf, rows, bucket);
    }
    Ok(buf)
}

fn emit_bucket(buf: &mut String, rows: &[Row], bucket: Bucket) {
    let mut mine: Vec<&Row> = rows.iter().filter(|r| r.bucket == bucket).collect();
    mine.sort_by(|a, b| {
        b.author_new_epoch
            .cmp(&a.author_new_epoch)
            .then_with(|| a.branch.cmp(&b.branch))
    });
    buf.push_str(&format!("* {}\n", bucket.title()));
    buf.push_str(&header_row());
    buf.push('\n');
    buf.push_str(&separator(COLS.len()));
    buf.push('\n');
    for row in mine {
        buf.push_str(&data_row(row));
        buf.push('\n');
    }
    buf.push('\n');
}

fn header_row() -> String {
    let mut s = String::from("|");
    for col in COLS {
        s.push(' ');
        s.push_str(col);
        s.push_str(" |");
    }
    s
}

fn separator(n: usize) -> String {
    let mut s = String::from("|");
    for _ in 0..n {
        s.push_str("---|");
    }
    s
}

fn data_row(row: &Row) -> String {
    let ahead = row.ahead.to_string();
    let behind = row.behind.to_string();
    let cells = [
        escape_cell(&row.branch),
        escape_cell(row.sha.as_bytes()),
        escape_cell(row.base.as_bytes()),
        escape_cell(ahead.as_bytes()),
        escape_cell(behind.as_bytes()),
        escape_cell(row.author_old.as_bytes()),
        escape_cell(row.author_new.as_bytes()),
        escape_cell(row.grade.as_bytes()),
        escape_cell(row.check.as_bytes()),
        escape_cell(row.evidence.as_bytes()),
        escape_cell(row.applies.as_bytes()),
        escape_cell(row.conflicts.as_bytes()),
        escape_cell(row.dropped.as_bytes()),
        escape_cell(row.reason.as_bytes()),
    ];
    let mut s = String::from("|");
    for cell in &cells {
        s.push(' ');
        s.push_str(cell);
        s.push_str(" |");
    }
    s
}

fn parse_drops(text: &str) -> Result<(PathBuf, String, String, Vec<DropRow>), String> {
    let mut repo = String::new();
    let mut trunk = String::new();
    let mut remote = String::from("origin");
    let mut section = String::new();
    let mut header: Vec<String> = Vec::new();
    let mut drops = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#+") {
            if let Some((key, value)) = rest.split_once(':') {
                // The separator after the keyword is one ASCII space.
                // Unicode whitespace in the value is part of the name.
                let value = value.strip_prefix(' ').unwrap_or(value);
                match key {
                    "repo" => repo = header_text(value)?,
                    "trunk" => trunk = header_text(value)?,
                    "remote" => remote = header_text(value)?,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(title) = line.strip_prefix("* ") {
            section = title.trim().to_string();
            header.clear();
            continue;
        }
        if section != "DROP" || !line.starts_with('|') {
            continue;
        }
        let cells = cells(line);
        if cells.is_empty() || cells.iter().all(|c| !c.is_empty() && dash(c)) {
            continue;
        }
        if header.is_empty() {
            header = cells;
            continue;
        }
        let mut branch = Vec::new();
        let mut grade = String::new();
        let mut check = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let Some(name) = header.get(i) else {
                continue;
            };
            match name.as_str() {
                "branch" => branch = unescape_cell(cell)?,
                "grade" => grade = cell_string(cell)?,
                "check" => check = cell_string(cell)?,
                _ => {}
            }
        }
        if branch.is_empty() {
            return Err("DROP row has no branch".into());
        }
        drops.push(DropRow {
            branch,
            grade,
            check,
        });
    }
    if repo.is_empty() || trunk.is_empty() {
        return Err("plan is missing repo or trunk".into());
    }
    Ok((PathBuf::from(repo), trunk, remote, drops))
}

fn parse_order(text: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut section = String::new();
    let mut refs = Vec::new();
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("* ") {
            section = title.trim().to_string();
            continue;
        }
        if section != "ORDER" || line.is_empty() {
            continue;
        }
        let Some((_, rest)) = line.split_once(". ") else {
            continue;
        };
        // ASCII space only. U+00A0 is a legal byte in a ref name.
        let mut parts = rest.split(' ').filter(|part| !part.is_empty());
        let Some(_verb) = parts.next() else {
            continue;
        };
        let Some(branch) = parts.next() else {
            continue;
        };
        refs.push(unescape_cell(branch)?);
    }
    Ok(refs)
}

fn order_holds(repo: &Path, text: &str, failed: &mut Vec<String>) -> Result<(), String> {
    let order = parse_order(text)?;
    let mut tips = Vec::with_capacity(order.len());
    for branch in order {
        let full = head_ref(&branch);
        if !branch_exists(repo, &full)? {
            failed.push(format!("order does not hold: {}", escape_cell(&branch)));
            continue;
        }
        tips.push((branch, live_sha(repo, &full)?));
    }
    for i in 0..tips.len() {
        for j in (i + 1)..tips.len() {
            if gitutil::strict_ancestor(repo, &tips[j].1, &tips[i].1) {
                failed.push(format!(
                    "order puts {} after {}",
                    escape_cell(&tips[j].0),
                    escape_cell(&tips[i].0)
                ));
            }
        }
    }
    Ok(())
}

fn cells(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            cur.push(b'\\');
            cur.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if bytes[i] == b'|' {
            out.push(trim_pad(&cur));
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(bytes[i]);
        i += 1;
    }
    out.push(trim_pad(&cur));
    if out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

fn trim_pad(raw: &[u8]) -> String {
    let mut start = 0;
    let mut end = raw.len();
    while start < end && (raw[start] == b' ' || raw[start] == b'\t') {
        start += 1;
    }
    while end > start && (raw[end - 1] == b' ' || raw[end - 1] == b'\t') {
        end -= 1;
    }
    match std::str::from_utf8(&raw[start..end]) {
        Ok(text) => text.to_string(),
        Err(_) => String::new(),
    }
}

fn cell_string(cell: &str) -> Result<String, String> {
    let raw = unescape_cell(cell)?;
    String::from_utf8(raw).map_err(|_| "plan cell is not utf-8".to_string())
}

fn header_text(value: &str) -> Result<String, String> {
    let raw = unescape_cell(value)?;
    String::from_utf8(raw).map_err(|_| "plan header is not utf-8".to_string())
}

fn unescape_cell(cell: &str) -> Result<Vec<u8>, String> {
    let b = cell.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        if i + 1 >= b.len() {
            return Err("truncated escape in plan cell".into());
        }
        i += 1;
        match b[i] {
            b'\\' => out.push(b'\\'),
            b'|' => out.push(b'|'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'0'..=b'7' => {
                if i + 2 >= b.len() {
                    return Err("truncated octal escape in plan cell".into());
                }
                let digits = &b[i..i + 3];
                if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                    return Err("bad octal escape in plan cell".into());
                }
                let val = u16::from(digits[0] - b'0') * 64
                    + u16::from(digits[1] - b'0') * 8
                    + u16::from(digits[2] - b'0');
                if val > 255 {
                    return Err("octal escape out of range".into());
                }
                out.push(val as u8);
                i += 2;
            }
            _ => return Err("unknown escape in plan cell".into()),
        }
        i += 1;
    }
    Ok(out)
}

fn escape_cell(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let end = raw.len().saturating_sub(1);
    let mut i = 0;
    while i < raw.len() {
        // Escape a leading or trailing space so cell padding can be trimmed.
        if raw[i] == b' ' && (i == 0 || i == end) {
            push_octal(&mut out, raw[i]);
            i += 1;
            continue;
        }
        match raw[i] {
            b'\\' => {
                out.push_str("\\\\");
                i += 1;
            }
            b'|' => {
                out.push_str("\\|");
                i += 1;
            }
            b'\n' => {
                out.push_str("\\n");
                i += 1;
            }
            b'\r' => {
                out.push_str("\\r");
                i += 1;
            }
            b'\t' => {
                out.push_str("\\t");
                i += 1;
            }
            b if b < 0x20 || b == 0x7f => {
                push_octal(&mut out, b);
                i += 1;
            }
            b if b < 0x80 => {
                out.push(b as char);
                i += 1;
            }
            _ => {
                let width = utf8_len(raw[i]);
                if width > 1
                    && i + width <= raw.len()
                    && std::str::from_utf8(&raw[i..i + width]).is_ok()
                {
                    out.push_str(std::str::from_utf8(&raw[i..i + width]).unwrap());
                    i += width;
                } else {
                    push_octal(&mut out, raw[i]);
                    i += 1;
                }
            }
        }
    }
    out
}

fn push_octal(out: &mut String, b: u8) {
    out.push('\\');
    out.push(char::from(b'0' + (b >> 6)));
    out.push(char::from(b'0' + ((b >> 3) & 7)));
    out.push(char::from(b'0' + (b & 7)));
}

fn utf8_len(b: u8) -> usize {
    match b {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

fn dash(cell: &str) -> bool {
    cell.chars().all(|c| c == '-' || c == '+')
}

fn selected_heads(
    repo: &Path,
    trunk_ref: &[u8],
    include: &[String],
    exclude: &[String],
) -> Result<Vec<Head>, String> {
    let mut heads = list_heads(repo, "refs/heads", trunk_ref, include, exclude)?;
    heads.extend(list_heads(
        repo,
        "refs/remotes",
        trunk_ref,
        include,
        exclude,
    )?);
    heads.sort_by(|a, b| a.name.cmp(&b.name));
    heads.dedup_by(|a, b| a.name == b.name);
    Ok(heads)
}

fn list_heads(
    repo: &Path,
    prefix: &str,
    trunk_ref: &[u8],
    include: &[String],
    exclude: &[String],
) -> Result<Vec<Head>, String> {
    let args = [
        OsStr::new("for-each-ref"),
        OsStr::new("--format=%(objectname)%00%(refname)"),
        OsStr::new("--end-of-options"),
        OsStr::new(prefix),
    ];
    let out = gitutil::git_raw(repo, &args);
    if out.status != 0 {
        return Err(format!("for-each-ref: {}", stderr_text(&out.stderr)));
    }
    let mut stdout = out.stdout;
    while stdout.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        stdout.pop();
    }
    let mut heads = Vec::new();
    if stdout.is_empty() {
        return Ok(heads);
    }
    let prefix_bytes = prefix.as_bytes();
    for line in stdout.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Some(nul) = line.iter().position(|b| *b == 0) else {
            return Err("head line is not a sha and a ref".into());
        };
        let (sha, name) = (&line[..nul], &line[nul + 1..]);
        if !name.starts_with(prefix_bytes) {
            continue;
        }
        if remote_head_symref(name) {
            continue;
        }
        if sha.len() != 40 || !sha.iter().all(u8::is_ascii_hexdigit) {
            return Err(format!("head {} is not a commit sha", escape_cell(name)));
        }
        if !wanted(name, trunk_ref, include, exclude) {
            continue;
        }
        let sha = std::str::from_utf8(sha)
            .map_err(|_| format!("head {} is not a commit sha", escape_cell(name)))?;
        heads.push(Head {
            name: name.to_vec(),
            sha: sha.to_string(),
        });
    }
    Ok(heads)
}

/// `refs/remotes/<remote>/HEAD` is the symref, not a branch named HEAD.
fn remote_head_symref(name: &[u8]) -> bool {
    let Some(rest) = name.strip_prefix(b"refs/remotes/") else {
        return false;
    };
    let Some(slash) = rest.iter().position(|b| *b == b'/') else {
        return false;
    };
    &rest[slash + 1..] == b"HEAD"
}

fn wanted(full: &[u8], trunk_ref: &[u8], include: &[String], exclude: &[String]) -> bool {
    if full == trunk_ref {
        return true;
    }
    let name = short_of(full);
    if !include.is_empty() && !include.iter().any(|pat| glob_match(pat, name)) {
        return false;
    }
    !exclude.iter().any(|pat| glob_match(pat, name))
}

fn name_ok(name: &[u8]) -> bool {
    !name.is_empty()
        && !name.contains(&b' ')
        && !name.contains(&b'\n')
        && !name.contains(&b'\r')
        && !name.contains(&b'\t')
        && !name.contains(&b':')
}

fn short_of(name: &[u8]) -> &[u8] {
    if let Some(rest) = name.strip_prefix(b"refs/heads/") {
        return rest;
    }
    if let Some(rest) = name.strip_prefix(b"refs/remotes/") {
        if let Some(slash) = rest.iter().position(|b| *b == b'/') {
            return &rest[slash + 1..];
        }
    }
    name
}

fn head_ref(name: &[u8]) -> Vec<u8> {
    if name.starts_with(b"refs/") {
        name.to_vec()
    } else {
        let mut full = Vec::with_capacity(b"refs/heads/".len() + name.len());
        full.extend_from_slice(b"refs/heads/");
        full.extend_from_slice(name);
        full
    }
}

fn snapshot(heads: &[Head]) -> String {
    let mut order: Vec<&Head> = heads.iter().collect();
    order.sort_by(|a, b| a.name.cmp(&b.name).then(a.sha.cmp(&b.sha)));
    let mut hasher = Sha256::new();
    for head in order {
        hasher.update(&head.name);
        hasher.update(b" ");
        hasher.update(head.sha.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex(&hasher.finalize()))
}

fn stderr_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_matches([' ', '\t', '\n', '\r'])
        .to_string()
}

fn git_ref(repo: &Path, args: &[&str], spec: &[u8]) -> gitutil::GitRaw {
    let spec = OsStr::from_bytes(spec);
    let mut all: Vec<&OsStr> = args.iter().copied().map(OsStr::new).collect();
    all.push(spec);
    gitutil::git_raw(repo, &all)
}

fn merge_base(repo: &Path, left: &str, right: &str) -> Result<Option<String>, String> {
    let out = gitutil::git(repo, &["merge-base", "--end-of-options", left, right]);
    if out.status == 0 {
        let sha = out.stdout.trim();
        if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Some(sha.to_string()));
        }
        return Err(format!("merge-base {left} {right}: {sha}"));
    }
    if out.status == 1 && out.stderr.trim().is_empty() {
        return Ok(None);
    }
    Err(format!("merge-base {left} {right}: {}", out.stderr.trim()))
}

fn author_range(repo: &Path, trunk: &str, branch: &str) -> Result<(String, String, i64), String> {
    let range = format!("{trunk}..{branch}");
    let text = git_ok(
        repo,
        &["log", "--format=%at %aI", "--end-of-options", &range],
    )?;
    let mut dates = parse_stamps(&text)?;
    if dates.is_empty() {
        let tip = git_ok(
            repo,
            &["log", "-1", "--format=%at %aI", "--end-of-options", branch],
        )?;
        dates = parse_stamps(&tip)?;
    }
    if dates.is_empty() {
        return Err(format!("author date for {branch}"));
    }
    dates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let old = &dates[0];
    let new = &dates[dates.len() - 1];
    Ok((old.1.clone(), new.1.clone(), new.0))
}

fn parse_stamps(text: &str) -> Result<Vec<(i64, String)>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((epoch, display)) = line.split_once(' ') else {
            return Err(format!("author date {line}"));
        };
        let epoch = epoch
            .parse::<i64>()
            .map_err(|_| format!("author date {line}"))?;
        out.push((epoch, canonical_stamp(display)));
    }
    Ok(out)
}

/// One RFC 3339 spelling for a stamp read from git.
///
/// `%aI` is strict ISO 8601. Git before 2.45 writes a zero offset as
/// `+00:00`; Git 2.45 and later write `Z`, which is the form ISO
/// 8601-1:2020 and the `time` crate's `Rfc3339` formatter use. Other
/// offsets stay `±HH:MM`.
fn canonical_stamp(display: &str) -> String {
    let head = display
        .strip_suffix("+00:00")
        .or_else(|| display.strip_suffix("-00:00"))
        .or_else(|| display.strip_suffix('Z'))
        .or_else(|| display.strip_suffix('z'));
    match head {
        Some(head) => format!("{head}Z"),
        None => display.to_string(),
    }
}

fn cherry(repo: &Path, trunk: &str, branch: &str) -> Result<Vec<Cherry>, String> {
    let out = gitutil::git(repo, &["cherry", "--end-of-options", trunk, branch]);
    if out.status != 0 {
        return Err(format!("cherry {branch}: {}", out.stderr.trim()));
    }
    let text = out.stdout;
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let mark = parts.next().unwrap_or("");
        let sha = parts.next().unwrap_or("");
        if sha.len() != 40 {
            return Err(format!("cherry line for {branch}: {line}"));
        }
        let upstream = match mark {
            "-" => true,
            "+" => false,
            _ => return Err(format!("cherry mark {mark} for {branch}")),
        };
        rows.push(Cherry {
            upstream,
            sha: sha.to_string(),
        });
    }
    Ok(rows)
}

fn squash_hit(
    repo: &Path,
    base: &str,
    trunk: &str,
    branch: &str,
    patches: &mut PatchCache,
) -> Result<Option<(String, String)>, String> {
    let Some(want) = patch_id(repo, base, branch)? else {
        return Ok(None);
    };
    let listed = git_ok(
        repo,
        &[
            "rev-list",
            "--reverse",
            "--end-of-options",
            &format!("{base}..{trunk}"),
        ],
    )?;
    for commit in listed.lines().filter(|l| !l.is_empty()) {
        if commit_patch(repo, commit, patches)? == Some(want.clone()) {
            return Ok(Some((want, commit.to_string())));
        }
    }
    Ok(None)
}

fn commit_patch(
    repo: &Path,
    commit: &str,
    patches: &mut PatchCache,
) -> Result<Option<String>, String> {
    if let Some(id) = patches.get(commit) {
        return Ok(id.clone());
    }
    let id = match git_ok(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{commit}^"),
        ],
    ) {
        Ok(parent) => patch_id(repo, &parent, commit)?,
        Err(_) => None,
    };
    patches.insert(commit.to_string(), id.clone());
    Ok(id)
}

fn patch_id(repo: &Path, from: &str, to: &str) -> Result<Option<String>, String> {
    let diff = git_bytes(
        repo,
        &[
            "diff",
            "--no-color",
            "--binary",
            "--full-index",
            "--end-of-options",
            from,
            to,
        ],
    )?;
    if diff.status != 0 {
        return Err(format!(
            "diff {from} {to}: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        ));
    }
    if diff.stdout.is_empty() {
        return Ok(None);
    }
    let id = git_stdin(&["patch-id", "--stable"], &diff.stdout)?;
    if id.status != 0 {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&id.stdout);
    let hex = text.split_whitespace().next().unwrap_or("");
    if hex.len() != 40 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    Ok(Some(hex.to_string()))
}

fn merge_outcome(repo: &Path, trunk: &str, branch: &str) -> Result<Merge, String> {
    let (clean, tree, conflicts) = gitutil::merge_tree(repo, trunk, branch);
    if clean && tree.len() != 40 {
        return Err(format!("merge-tree {branch}"));
    }
    Ok(Merge {
        clean,
        tree,
        conflicts,
    })
}

fn branch_exists(repo: &Path, branch: &[u8]) -> Result<bool, String> {
    let spec = head_ref(branch);
    let out = git_ref(
        repo,
        &["rev-parse", "--verify", "--quiet", "--end-of-options"],
        &spec,
    );
    Ok(out.status == 0)
}

fn live_sha(repo: &Path, branch: &[u8]) -> Result<String, String> {
    let spec = head_ref(branch);
    let out = git_ref(repo, &["rev-parse", "--verify", "--end-of-options"], &spec);
    if out.status != 0 {
        return Err(format!("rev-parse: {}", stderr_text(&out.stderr)));
    }
    let sha = trim_ascii(&out.stdout);
    let sha = std::str::from_utf8(sha).map_err(|_| "rev-parse sha is not utf-8".to_string())?;
    if sha.len() != 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("rev-parse sha".into());
    }
    Ok(sha.to_string())
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && matches!(bytes[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    while end > start && matches!(bytes[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    &bytes[start..end]
}

fn locate(repo: &Path) -> Result<PathBuf, String> {
    if repo.exists() {
        fs::canonicalize(repo).map_err(|e| format!("repo: {e}"))
    } else {
        Ok(repo.to_path_buf())
    }
}

fn clone_bare(src: &Path, dst: &Path) -> Result<(), String> {
    let src = src
        .to_str()
        .ok_or_else(|| "repo path is not utf-8".to_string())?;
    let dst = dst
        .to_str()
        .ok_or_else(|| "store path is not utf-8".to_string())?;
    let out = gitutil::git(
        Path::new("."),
        &["clone", "--bare", "--no-local", "--", src, dst],
    );
    if out.status != 0 {
        return Err(format!("clone: {}", out.stderr.trim()));
    }
    copy_remotes(Path::new(src), Path::new(dst))
}

/// A bare clone drops `refs/remotes`. Copy them into the private store
/// when the source is an ordinary checkout.
fn copy_remotes(src: &Path, dst: &Path) -> Result<(), String> {
    let listed = gitutil::git(
        src,
        &["for-each-ref", "--format=%(refname)", "refs/remotes"],
    );
    if listed.status != 0 || listed.stdout.trim().is_empty() {
        return Ok(());
    }
    let src = src
        .to_str()
        .ok_or_else(|| "repo path is not utf-8".to_string())?;
    let out = gitutil::git(
        dst,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--",
            src,
            "+refs/remotes/*:refs/remotes/*",
        ],
    );
    if out.status != 0 {
        return Err(format!("fetch remotes: {}", out.stderr.trim()));
    }
    // git fetch exits 0 when it refuses a shallow-root update.
    let mut rejected = rejected_refs(&out.stderr);
    for name in rejected_refs(&out.stdout) {
        if !rejected.contains(&name) {
            rejected.push(name);
        }
    }
    if !rejected.is_empty() {
        return Err(format!(
            "fetch rejected {}; git fetch --unshallow",
            rejected.join(", ")
        ));
    }
    Ok(())
}

fn rejected_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("warning: rejected ") else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        if !name.is_empty() && !refs.iter().any(|have| have == name) {
            refs.push(name.to_string());
        }
    }
    refs
}

fn require_full_history(repo: &Path) -> Result<(), String> {
    let out = gitutil::git(repo, &["rev-parse", "--is-shallow-repository"]);
    if out.status == 0 && out.stdout.trim() == "true" {
        return Err("shallow repository; git fetch --unshallow".into());
    }
    Ok(())
}

fn trunk_ref(repo: &Path, trunk: &str, remote: &str) -> Result<Vec<u8>, String> {
    let local = format!("refs/heads/{trunk}");
    if verify_ref(repo, &local).is_some() {
        return Ok(local.into_bytes());
    }
    let via = format!("refs/remotes/{remote}/{trunk}");
    if verify_ref(repo, &via).is_some() {
        return Ok(via.into_bytes());
    }
    Err(format!("trunk {trunk} is not a head"))
}

fn trunk_tip(repo: &Path, trunk: &str, remote: &str) -> Result<String, String> {
    let local = format!("refs/heads/{trunk}");
    if let Some(sha) = verify_ref(repo, &local) {
        return Ok(sha);
    }
    let via = format!("refs/remotes/{remote}/{trunk}");
    if let Some(sha) = verify_ref(repo, &via) {
        return Ok(sha);
    }
    Err(format!("trunk {trunk} is not a head"))
}

fn verify_ref(repo: &Path, spec: &str) -> Option<String> {
    let out = gitutil::git(repo, &["rev-parse", "--verify", "--end-of-options", spec]);
    if out.status != 0 {
        return None;
    }
    let sha = out.stdout.trim();
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

fn scratch_dir() -> Result<Scratch, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kitline-triage-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(Scratch { path })
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

fn git_bytes(repo: &Path, args: &[&str]) -> Result<Out, String> {
    let mut cmd = command();
    cmd.arg("--no-pager").arg("-C").arg(repo).args(args);
    spawn(cmd, None)
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = gitutil::git(repo, args);
    if out.status != 0 {
        return Err(format!("{}: {}", args.join(" "), out.stderr.trim()));
    }
    Ok(out.stdout.trim().to_string())
}

fn git_stdin(args: &[&str], stdin: &[u8]) -> Result<Out, String> {
    let mut cmd = command();
    cmd.arg("--no-pager").args(args);
    spawn(cmd, Some(stdin))
}

fn command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn spawn(mut cmd: Command, stdin: Option<&[u8]>) -> Result<Out, String> {
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("git: {e}"))?;
    if let Some(bytes) = stdin {
        let mut pipe = child
            .stdin
            .take()
            .ok_or_else(|| "git stdin was not piped".to_string())?;
        pipe.write_all(bytes).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    Ok(Out {
        status: out.status.code().unwrap_or(1),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

fn glob_match(pat: &str, text: &[u8]) -> bool {
    glob_rec(pat.as_bytes(), text)
}

fn glob_rec(pat: &[u8], text: &[u8]) -> bool {
    let mut i = 0;
    let mut j = 0;
    let mut star_i: Option<usize> = None;
    let mut star_j = 0;
    while j < text.len() {
        if i < pat.len() && (pat[i] == b'?' || pat[i] == text[j]) {
            i += 1;
            j += 1;
        } else if i < pat.len() && pat[i] == b'*' {
            star_i = Some(i);
            star_j = j;
            i += 1;
        } else if let Some(saved) = star_i {
            i = saved + 1;
            star_j += 1;
            j = star_j;
        } else {
            return false;
        }
    }
    while i < pat.len() && pat[i] == b'*' {
        i += 1;
    }
    i == pat.len()
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(H[(b >> 4) as usize] as char);
        out.push(H[(b & 0xf) as usize] as char);
    }
    out
}

struct Land {
    text: String,
    lines: Vec<String>,
    stacks: usize,
    order: usize,
}

fn land_candidate(row: &Row) -> bool {
    matches!(row.bucket, Bucket::Rest | Bucket::Rebase)
        && (row.grade == "open" || row.grade == "partial")
}

fn land_verb(row: &Row) -> &'static str {
    if row.grade == "open" && row.applies == "yes" && row.bucket == Bucket::Rest {
        "land"
    } else {
        "rebase"
    }
}

/// Parents first. Graphite's `gt log short` lists the tip first; this list
/// is that sequence reversed, the same direction as the parent-before-child
/// tree `git machete discover` prints.
fn land_plan(repo: &Path, rows: &[Row]) -> Result<Land, String> {
    let cands: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| land_candidate(row))
        .map(|(i, _)| i)
        .collect();
    if cands.is_empty() {
        return Ok(Land {
            text: "#+stacks: 0\n#+order: 0\n\n* STACKS\n\n* ORDER\n\n".into(),
            lines: Vec::new(),
            stacks: 0,
            order: 0,
        });
    }
    let mut shas: Vec<String> = cands.iter().map(|&i| rows[i].sha.clone()).collect();
    shas.sort();
    shas.dedup();
    let parents = parent_map(repo, &shas)?;
    let mut at: HashMap<String, Vec<usize>> = HashMap::new();
    for (local, &row_i) in cands.iter().enumerate() {
        at.entry(rows[row_i].sha.clone()).or_default().push(local);
    }
    let mut memo: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut under = Vec::with_capacity(cands.len());
    for local in 0..cands.len() {
        let mut set = tips_under(&rows[cands[local]].sha, &parents, &at, &mut memo);
        set.remove(&local);
        under.push(set);
    }
    let mut imm = immediate(&under);
    // Land before rebase, then oldest. A parent stays ahead of its child
    // because the child is not ready until the parent has been listed.
    let keys: Vec<(u8, i64, Vec<u8>)> = cands
        .iter()
        .map(|&i| {
            let rank = if land_verb(&rows[i]) == "land" { 0 } else { 1 };
            (rank, rows[i].author_new_epoch, rows[i].branch.clone())
        })
        .collect();
    for group in &mut imm {
        group.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
        group.dedup();
    }
    let order = topo(&imm, &keys);
    let number = numbers(&order, cands.len());
    let lines = order
        .iter()
        .map(|&local| {
            step_text(
                number[local],
                &rows[cands[local]],
                &imm[local],
                rows,
                &cands,
            )
        })
        .collect::<Vec<_>>();
    let (tree, stacks) = stack_tree(rows, &cands, &imm, &order, &number);
    let mut text = String::new();
    text.push_str(&format!("#+stacks: {stacks}\n"));
    text.push_str(&format!("#+order: {}\n\n", lines.len()));
    text.push_str("* STACKS\n\n");
    text.push_str(&tree);
    if !tree.ends_with('\n') {
        text.push('\n');
    }
    text.push('\n');
    text.push_str("* ORDER\n\n");
    for line in &lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push('\n');
    Ok(Land {
        text,
        order: lines.len(),
        stacks,
        lines,
    })
}

fn step_text(n: usize, row: &Row, parents: &[usize], rows: &[Row], cands: &[usize]) -> String {
    let also = if parents.len() < 2 {
        String::new()
    } else {
        let rest = parents[1..]
            .iter()
            .map(|&p| escape_cell(&rows[cands[p]].branch))
            .collect::<Vec<_>>()
            .join(" ");
        format!(" also {rest}")
    };
    format!("{n}. {} {}{also}", land_verb(row), escape_cell(&row.branch))
}

fn parent_map(repo: &Path, shas: &[String]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut map = HashMap::new();
    for chunk in shas.chunks(400) {
        let mut args = vec!["rev-list", "--parents", "--end-of-options"];
        for sha in chunk {
            args.push(sha.as_str());
        }
        let text = git_ok(repo, &args)?;
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(sha) = parts.next() else {
                continue;
            };
            if sha.len() != 40 {
                continue;
            }
            let ps = parts
                .filter(|p| p.len() == 40)
                .map(str::to_string)
                .collect();
            map.entry(sha.to_string()).or_insert(ps);
        }
    }
    Ok(map)
}

fn tips_under(
    start: &str,
    parents: &HashMap<String, Vec<String>>,
    at: &HashMap<String, Vec<usize>>,
    memo: &mut HashMap<String, HashSet<usize>>,
) -> HashSet<usize> {
    if let Some(got) = memo.get(start) {
        return got.clone();
    }
    let mut color: HashMap<String, u8> = HashMap::new();
    let mut stack = vec![start.to_string()];
    while let Some(sha) = stack.last().cloned() {
        if memo.contains_key(&sha) {
            stack.pop();
            continue;
        }
        if color.get(&sha).copied().unwrap_or(0) == 0 {
            color.insert(sha.clone(), 1);
            if let Some(ps) = parents.get(&sha) {
                for parent in ps {
                    if !memo.contains_key(parent) && color.get(parent).copied().unwrap_or(0) == 0 {
                        stack.push(parent.clone());
                    }
                }
            }
            continue;
        }
        let mut set = HashSet::new();
        if let Some(ps) = parents.get(&sha) {
            for parent in ps {
                if let Some(tips) = at.get(parent) {
                    set.extend(tips.iter().copied());
                }
                if let Some(below) = memo.get(parent) {
                    set.extend(below.iter().copied());
                }
            }
        }
        memo.insert(sha, set);
        stack.pop();
    }
    memo.get(start).cloned().unwrap_or_default()
}

fn immediate(under: &[HashSet<usize>]) -> Vec<Vec<usize>> {
    let mut imm = vec![Vec::new(); under.len()];
    for (i, below) in under.iter().enumerate() {
        for &j in below {
            let nested = below.iter().any(|&k| k != j && under[k].contains(&j));
            if !nested {
                imm[i].push(j);
            }
        }
    }
    imm
}

fn topo(imm: &[Vec<usize>], keys: &[(u8, i64, Vec<u8>)]) -> Vec<usize> {
    let n = imm.len();
    let mut indeg = vec![0usize; n];
    let mut children = vec![Vec::new(); n];
    for (child, ps) in imm.iter().enumerate() {
        indeg[child] = ps.len();
        for &parent in ps {
            children[parent].push(child);
        }
    }
    let key = |i: usize| (keys[i].0, keys[i].1, keys[i].2.clone(), i);
    let mut ready: BTreeSet<(u8, i64, Vec<u8>, usize)> = BTreeSet::new();
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        if indeg[i] == 0 {
            ready.insert(key(i));
        }
    }
    let mut order = Vec::with_capacity(n);
    while let Some(item) = ready.pop_first() {
        let i = item.3;
        order.push(i);
        for &child in &children[i] {
            indeg[child] -= 1;
            if indeg[child] == 0 {
                ready.insert(key(child));
            }
        }
    }
    if order.len() != n {
        let mut left: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
        left.sort_by_key(|&i| key(i));
        order.extend(left);
    }
    order
}

fn numbers(order: &[usize], n: usize) -> Vec<usize> {
    let mut number = vec![0usize; n];
    for (i, &local) in order.iter().enumerate() {
        number[local] = i + 1;
    }
    number
}

fn stack_tree(
    rows: &[Row],
    cands: &[usize],
    imm: &[Vec<usize>],
    order: &[usize],
    number: &[usize],
) -> (String, usize) {
    let n = cands.len();
    let mut children = vec![Vec::new(); n];
    for (child, ps) in imm.iter().enumerate() {
        if let Some(&first) = ps.first() {
            children[first].push(child);
        }
    }
    for kids in &mut children {
        kids.sort_by_key(|&child| number[child]);
        kids.dedup();
    }
    let mut buf = String::new();
    let mut printed = HashSet::new();
    let mut stacks = 0usize;
    let mut roots: Vec<usize> = (0..n).filter(|&i| imm[i].is_empty()).collect();
    roots.sort_by_key(|&i| number[i]);
    for root in roots {
        emit_stack(
            root,
            0,
            rows,
            cands,
            imm,
            &children,
            number,
            &mut printed,
            &mut buf,
            &mut stacks,
        );
    }
    for &local in order {
        if !printed.contains(&local) {
            emit_stack(
                local,
                0,
                rows,
                cands,
                imm,
                &children,
                number,
                &mut printed,
                &mut buf,
                &mut stacks,
            );
        }
    }
    (buf, stacks)
}

#[allow(clippy::too_many_arguments)]
fn emit_stack(
    local: usize,
    depth: usize,
    rows: &[Row],
    cands: &[usize],
    imm: &[Vec<usize>],
    children: &[Vec<usize>],
    number: &[usize],
    printed: &mut HashSet<usize>,
    buf: &mut String,
    stacks: &mut usize,
) {
    if !printed.insert(local) {
        return;
    }
    if depth == 0 {
        *stacks += 1;
    }
    let text = step_text(number[local], &rows[cands[local]], &imm[local], rows, cands);
    buf.push_str(&"  ".repeat(depth));
    buf.push_str("- ");
    buf.push_str(&text);
    buf.push('\n');
    for &child in &children[local] {
        emit_stack(
            child,
            depth + 1,
            rows,
            cands,
            imm,
            children,
            number,
            printed,
            buf,
            stacks,
        );
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    fn keys(epochs: &[i64]) -> Vec<(u8, i64, Vec<u8>)> {
        epochs
            .iter()
            .enumerate()
            .map(|(i, epoch)| (0, *epoch, vec![b'a' + i as u8]))
            .collect()
    }

    #[test]
    fn chain_lands_base_first() {
        let under = vec![HashSet::new(), HashSet::from([0]), HashSet::from([0, 1])];
        let imm = immediate(&under);
        assert_eq!(imm[2], vec![1]);
        assert_eq!(topo(&imm, &keys(&[1, 2, 3])), vec![0, 1, 2]);
    }

    #[test]
    fn fork_orders_children_by_age() {
        let under = vec![HashSet::new(), HashSet::from([0]), HashSet::from([0])];
        let imm = immediate(&under);
        assert_eq!(topo(&imm, &keys(&[1, 3, 2])), vec![0, 2, 1]);
    }

    #[test]
    fn two_parents_both_precede_the_child() {
        let under = vec![HashSet::new(), HashSet::new(), HashSet::from([0, 1])];
        let imm = immediate(&under);
        assert_eq!(imm[2].len(), 2);
        let order = topo(&imm, &keys(&[2, 1, 3]));
        assert_eq!(order.last().copied(), Some(2));
        assert!(order.iter().position(|i| *i == 0).unwrap() < 2);
        assert!(order.iter().position(|i| *i == 1).unwrap() < 2);
    }
}
