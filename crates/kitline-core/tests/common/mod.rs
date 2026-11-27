#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Tmp {
    path: PathBuf,
}

impl Tmp {
    pub fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("kitline-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct Repo {
    pub path: PathBuf,
}

pub fn repo(parent: &Path) -> Repo {
    let path = parent.join("repo");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"], &[]);
    git(&path, &["config", "user.name", "Kit Tester"], &[]);
    git(&path, &["config", "user.email", "kit@example.com"], &[]);
    Repo { path }
}

pub fn git(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = raw(Some(repo), args, env, None);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn git_out(repo: &Path, args: &[&str]) -> Vec<u8> {
    raw(Some(repo), args, &[], None).stdout
}

pub fn git_in(repo: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> String {
    let out = raw(Some(repo), args, env, Some(stdin));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn git_here(args: &[&str]) -> String {
    let out = raw(None, args, &[], None);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn raw(repo: Option<&Path>, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Output {
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("GIT_AUTHOR_NAME", "Kit Tester")
        .env("GIT_AUTHOR_EMAIL", "kit@example.com")
        .env("GIT_COMMITTER_NAME", "Kit Tester")
        .env("GIT_COMMITTER_EMAIL", "kit@example.com")
        .env("GIT_EDITOR", "true");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.arg("--no-pager").arg("-c").arg("commit.gpgsign=false");
    if let Some(repo) = repo {
        cmd.arg("-C").arg(repo);
    }
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(bytes) = stdin {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    if !out.status.success() {
        panic!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    out
}

pub fn write(repo: &Path, name: &str, bytes: &str) {
    std::fs::write(repo.join(name), bytes).unwrap();
}

pub fn commit(repo: &Path, message: &str, author_date: &str, committer_date: &str) {
    git(repo, &["add", "-A"], &[]);
    git_in(
        repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--cleanup=verbatim",
            "-F",
            "-",
        ],
        &[
            ("GIT_AUTHOR_DATE", author_date),
            ("GIT_COMMITTER_DATE", committer_date),
        ],
        message.as_bytes(),
    );
}

pub fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"], &[])
}
