//! Triage phases 1, 2 and 6.
//!
//! Phase 1 inventories every head. Phase 2 grades what is already
//! upstream with patch-id, cherry and merge-base. Phase 6 records
//! freshness. The plan lists KEEP, DROP, REBASE, the remaining heads, and
//! an unrelated head as UNSORTED. Rows use `refs/heads/...`. Git commands
//! take the commit sha. `triage verify` re-reads that ref and re-runs each
//! DROP check.

use crate::gitutil;
use clap::Parser;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

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
    name: String,
    sha: String,
}

struct Ctx<'a> {
    repo: &'a Path,
    trunk: &'a str,
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
    branch: String,
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
    branch: String,
    grade: String,
    check: String,
}

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    let scratch = scratch_dir()?;
    let bare = scratch.path.join("bare.git");
    clone_bare(&repo, &bare)?;
    let trunk_ref = format!("refs/heads/{}", cli.trunk);
    let trunk_sha = git_ok(
        &bare,
        &["rev-parse", "--verify", "--end-of-options", &trunk_ref],
    )
    .map_err(|_| format!("trunk {} is not a head", cli.trunk))?;
    let trunk_tree = git_ok(
        &bare,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{trunk_sha}^{{tree}}"),
        ],
    )?;
    let heads = selected_heads(&bare, &cli.trunk, &cli.include, &cli.exclude)?;
    if !heads.iter().any(|h| short_of(&h.name) == cli.trunk) {
        return Err(format!("trunk {} is not a head", cli.trunk));
    }
    let snap = snapshot(&heads);
    let ctx = Ctx {
        repo: &bare,
        trunk: &cli.trunk,
        trunk_sha: &trunk_sha,
        trunk_tree: &trunk_tree,
    };
    let mut rows = Vec::with_capacity(heads.len());
    for head in &heads {
        let short = short_of(&head.name);
        let kept = short == cli.trunk || cli.keep.iter().any(|pat| glob_match(pat, short));
        rows.push(classify(&ctx, head, kept)?);
    }
    let text = render(&repo, &cli.trunk, &trunk_sha, &snap, &rows)?;
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
    println!("wrote {}", cli.out.display());
    Ok(())
}

fn verify(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("bad plan: {e}"))?;
    let (repo, trunk, drops) = parse_drops(&text)?;
    let scratch = scratch_dir()?;
    let bare = scratch.path.join("bare.git");
    clone_bare(&repo, &bare)?;
    let trunk_ref = format!("refs/heads/{trunk}");
    let trunk_sha = git_ok(
        &bare,
        &["rev-parse", "--verify", "--end-of-options", &trunk_ref],
    )
    .map_err(|_| format!("trunk {trunk} is not a head"))?;
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
        trunk: &trunk,
        trunk_sha: &trunk_sha,
        trunk_tree: &trunk_tree,
    };
    let mut failed = Vec::new();
    for row in &drops {
        if row.grade != row.check || row.check.is_empty() {
            failed.push(format!("drop does not hold: {}", row.branch));
            continue;
        }
        let full = head_ref(&row.branch);
        if !branch_exists(&bare, &full)? {
            failed.push(format!("drop does not hold: {}", row.branch));
            continue;
        }
        // Re-read the live tip of the full ref. The sha in the plan is only
        // the evidence from plan time.
        let live = live_sha(&bare, &full)?;
        let holds = check_holds(&ctx, &live, &row.check)?;
        if !holds {
            failed.push(format!("drop does not hold: {}", row.branch));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("\n"))
    }
}

fn check_holds(ctx: &Ctx<'_>, sha: &str, check: &str) -> Result<bool, String> {
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
            let hit = squash_hit(ctx.repo, &base, ctx.trunk_sha, sha)?;
            Ok(hit.is_some() && noop)
        }
        "no-op" => Ok(noop),
        _ => Ok(false),
    }
}

fn classify(ctx: &Ctx<'_>, head: &Head, kept: bool) -> Result<Row, String> {
    let short = short_of(&head.name);
    if !name_ok(short) {
        return Err(format!("branch name {short} cannot be stored in the plan"));
    }
    let (ahead, behind) = gitutil::ahead_behind(ctx.repo, ctx.trunk_sha, &head.sha)
        .ok_or_else(|| format!("ahead/behind for {}", head.name))?;
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
    let decided = decide(ctx, head, kept, &base, &merge, noop)?;

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
) -> Result<Decision, String> {
    if kept {
        let reason = if short_of(&head.name) == ctx.trunk {
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
    if let Some((pid, commit)) = squash_hit(ctx.repo, base, ctx.trunk_sha, &head.sha)? {
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
    trunk_sha: &str,
    snap: &str,
    rows: &[Row],
) -> Result<String, String> {
    let repo = repo
        .to_str()
        .ok_or_else(|| "repo path is not utf-8".to_string())?;
    if repo.contains(['\n', '\r']) || trunk.contains(['\n', '\r']) {
        return Err("repo path or trunk contains a newline".into());
    }
    let mut buf = String::new();
    buf.push_str("#+title: kitline triage\n");
    buf.push_str(&format!("#+repo: {repo}\n"));
    buf.push_str(&format!("#+trunk: {trunk}\n"));
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
    let cells = [
        row.branch.as_str(),
        row.sha.as_str(),
        row.base.as_str(),
        &row.ahead.to_string(),
        &row.behind.to_string(),
        row.author_old.as_str(),
        row.author_new.as_str(),
        row.grade.as_str(),
        row.check.as_str(),
        row.evidence.as_str(),
        row.applies.as_str(),
        row.conflicts.as_str(),
        row.dropped.as_str(),
        row.reason.as_str(),
    ];
    let mut s = String::from("|");
    for cell in cells {
        s.push(' ');
        s.push_str(&sanitize(cell));
        s.push_str(" |");
    }
    s
}

fn sanitize(s: &str) -> String {
    s.replace(['|', '\n', '\r'], " ")
}

fn parse_drops(text: &str) -> Result<(PathBuf, String, Vec<DropRow>), String> {
    let mut repo = String::new();
    let mut trunk = String::new();
    let mut section = String::new();
    let mut header: Vec<String> = Vec::new();
    let mut drops = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#+") {
            if let Some((key, value)) = rest.split_once(':') {
                match key.trim() {
                    "repo" => repo = value.trim().to_string(),
                    "trunk" => trunk = value.trim().to_string(),
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
        let mut branch = String::new();
        let mut grade = String::new();
        let mut check = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let Some(name) = header.get(i) else {
                continue;
            };
            match name.as_str() {
                "branch" => branch = cell.clone(),
                "grade" => grade = cell.clone(),
                "check" => check = cell.clone(),
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
    Ok((PathBuf::from(repo), trunk, drops))
}

fn cells(line: &str) -> Vec<String> {
    let mut out: Vec<String> = line.split('|').map(|s| s.trim().to_string()).collect();
    if out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

fn dash(cell: &str) -> bool {
    cell.chars().all(|c| c == '-' || c == '+')
}

fn selected_heads(
    repo: &Path,
    trunk: &str,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<Head>, String> {
    let out = gitutil::git(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname)%00%(refname)",
            "--end-of-options",
            "refs/heads",
        ],
    );
    if out.status != 0 {
        return Err(format!("for-each-ref: {}", out.stderr.trim()));
    }
    let mut heads = Vec::new();
    let text = out.stdout.trim_end_matches(['\r', '\n']);
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let Some((sha, name)) = line.split_once('\0') else {
            return Err(format!("head line is not a sha and a ref: {line}"));
        };
        if !name.starts_with("refs/heads/") {
            continue;
        }
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("head {name} is not a commit sha"));
        }
        if !wanted(short_of(name), trunk, include, exclude) {
            continue;
        }
        heads.push(Head {
            name: name.to_string(),
            sha: sha.to_string(),
        });
    }
    heads.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(heads)
}

fn wanted(name: &str, trunk: &str, include: &[String], exclude: &[String]) -> bool {
    if name == trunk {
        return true;
    }
    if !include.is_empty() && !include.iter().any(|pat| glob_match(pat, name)) {
        return false;
    }
    !exclude.iter().any(|pat| glob_match(pat, name))
}

fn name_ok(name: &str) -> bool {
    !name.is_empty() && !name.contains(['|', ' ', '\n', '\r', '\t']) && !name.contains(':')
}

fn short_of(name: &str) -> &str {
    name.strip_prefix("refs/heads/").unwrap_or(name)
}

fn head_ref(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    }
}

fn snapshot(heads: &[Head]) -> String {
    let mut lines: Vec<String> = heads
        .iter()
        .map(|h| format!("{} {}", h.name, h.sha))
        .collect();
    lines.sort();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex(&hasher.finalize()))
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
        out.push((epoch, display.to_string()));
    }
    Ok(out)
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
        let parent = match git_ok(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{commit}^"),
            ],
        ) {
            Ok(parent) => parent,
            Err(_) => continue,
        };
        if patch_id(repo, &parent, commit)? == Some(want.clone()) {
            return Ok(Some((want, commit.to_string())));
        }
    }
    Ok(None)
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

fn branch_exists(repo: &Path, branch: &str) -> Result<bool, String> {
    let spec = head_ref(branch);
    let out = gitutil::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    );
    Ok(out.status == 0)
}

fn live_sha(repo: &Path, branch: &str) -> Result<String, String> {
    let spec = head_ref(branch);
    git_ok(repo, &["rev-parse", "--verify", "--end-of-options", &spec])
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
    Ok(())
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

fn glob_match(pat: &str, text: &str) -> bool {
    glob_rec(pat.as_bytes(), text.as_bytes())
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
