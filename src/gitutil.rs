use std::path::Path;
use std::process::Command;

pub struct GitOut {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn git(dir: &Path, args: &[&str]) -> GitOut {
    let out = Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match out {
        Ok(out) => GitOut {
            status: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        },
        Err(err) => GitOut {
            status: 1,
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

pub fn ok(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git(dir, args);
    if out.status == 0 {
        Some(out.stdout.trim().to_string())
    } else {
        None
    }
}

pub fn rev(dir: &Path, spec: &str) -> Option<String> {
    ok(dir, &["rev-parse", "--verify", spec])
}

pub fn tree_of(dir: &Path, spec: &str) -> Option<String> {
    rev(dir, &format!("{spec}^{{tree}}"))
}

pub fn is_ancestor(dir: &Path, older: &str, newer: &str) -> bool {
    if older.is_empty() || newer.is_empty() {
        return false;
    }
    git(dir, &["merge-base", "--is-ancestor", older, newer]).status == 0
}

pub fn strict_ancestor(dir: &Path, older: &str, newer: &str) -> bool {
    !older.is_empty() && older != newer && is_ancestor(dir, older, newer)
}

pub fn ahead_behind(dir: &Path, target: &str, head: &str) -> Option<(u64, u64)> {
    if target.is_empty() || head.is_empty() {
        return None;
    }
    let spec = format!("{target}...{head}");
    let text = ok(dir, &["rev-list", "--left-right", "--count", &spec])?;
    let mut parts = text.split_whitespace();
    let behind = parts.next()?.parse().ok()?;
    let ahead = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

pub fn show_text(dir: &Path, rev: &str, path: &str) -> Option<String> {
    if rev.is_empty() {
        return None;
    }
    let spec = format!("{rev}:{path}");
    ok(dir, &["show", &spec])
}

pub fn list_files(dir: &Path, rev: &str) -> Vec<String> {
    if rev.is_empty() {
        return Vec::new();
    }
    ok(dir, &["ls-tree", "-r", "--name-only", rev])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn ref_exists(dir: &Path, name: &str) -> bool {
    git(
        dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .status
        == 0
        || git(
            dir,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/tags/{name}"),
            ],
        )
        .status
            == 0
}

pub fn tag_exists(dir: &Path, name: &str) -> bool {
    git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{name}"),
        ],
    )
    .status
        == 0
}

pub fn diff_names(dir: &Path, base: &str, head: &str) -> Vec<String> {
    if base.is_empty() || head.is_empty() {
        return Vec::new();
    }
    ok(dir, &["diff", "--name-only", base, head])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn merge_tree(dir: &Path, left: &str, right: &str) -> (bool, Vec<String>) {
    if left.is_empty() || right.is_empty() {
        return (true, Vec::new());
    }
    let out = git(
        dir,
        &["merge-tree", "--write-tree", "--name-only", left, right],
    );
    if out.status == 0 {
        return (true, Vec::new());
    }
    let mut files = Vec::new();
    for line in out.stdout.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("Auto-merging")
            || t.starts_with("CONFLICT")
            || t.contains(' ')
        {
            continue;
        }
        if t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        files.push(t.to_string());
    }
    (false, files)
}

pub fn commits_on(dir: &Path, base: &str, tip: &str) -> Vec<String> {
    if tip.is_empty() {
        return Vec::new();
    }
    let range = if base.is_empty() {
        tip.to_string()
    } else {
        format!("{base}..{tip}")
    };
    ok(dir, &["rev-list", &range])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn commit_message(dir: &Path, sha: &str) -> String {
    ok(dir, &["log", "-1", "--format=%B", sha]).unwrap_or_default()
}

pub fn bundle_prereqs(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut prereq = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            break;
        }
        if line.first() == Some(&b'#') {
            continue;
        }
        let s = String::from_utf8_lossy(line);
        if let Some(rest) = s.strip_prefix('-') {
            if let Some(sha) = rest.split_whitespace().next() {
                if !sha.is_empty() {
                    prereq.push(sha.to_string());
                }
            }
        }
    }
    prereq
}

pub fn verify_bundle(repo: &Path, bundle: &Path) -> Result<(), String> {
    let prereq = bundle_prereqs(bundle);
    let out = git(repo, &["bundle", "verify", &bundle.to_string_lossy()]);
    if out.status == 0 {
        return Ok(());
    }
    let sha = prereq.join(" ");
    Err(format!(
        "bundle prerequisite missing: {sha}\n{}{}",
        out.stderr, out.stdout
    ))
}

pub fn latest_tag(dir: &Path) -> Option<String> {
    let text = ok(dir, &["tag", "--sort=-v:refname"])?;
    text.lines().next().map(str::to_string)
}

pub fn expand(dir: &Path, spec: &str) -> String {
    rev(dir, spec).unwrap_or_else(|| spec.to_string())
}
