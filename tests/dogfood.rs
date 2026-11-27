use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("kitline-dogfood-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_at(dir: &Path, args: &[&str], date: &str) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "rgoswami@ieee.org"]);
    git(dir, &["config", "user.name", "Rohit Goswami"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(dir: &Path, name: &str, body: &str, msg: &str, date: &str) {
    fs::write(dir.join(name), body).unwrap();
    git(dir, &["add", name]);
    git_at(dir, &["commit", "-qm", msg], date);
}

fn triage(repo: &Path, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args([
            "triage",
            repo.to_str().unwrap(),
            "--trunk",
            "main",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn order_lines(text: &str) -> Vec<String> {
    let mut on = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line == "* ORDER" {
            on = true;
            continue;
        }
        if on && line.starts_with("* ") {
            break;
        }
        if on && !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn stacks_section(text: &str) -> String {
    let mut on = false;
    let mut out = String::new();
    for line in text.lines() {
        if line == "* STACKS" {
            on = true;
            continue;
        }
        if on && line.starts_with("* ") {
            break;
        }
        if on {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[test]
fn stack_orders_the_base_before_the_child_and_drops_the_ancestor() {
    let root = tmp();
    let repo = root.join("repo");
    init(&repo);
    commit_file(&repo, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    let root_sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "gone", &root_sha]);
    commit_file(&repo, "README", "two\n", "main", "2020-01-02T00:00:00Z");
    git(&repo, &["checkout", "-q", "-b", "base"]);
    commit_file(&repo, "a.txt", "a\n", "base", "2020-01-03T00:00:00Z");
    git(&repo, &["checkout", "-q", "-b", "topic"]);
    commit_file(&repo, "b.txt", "b\n", "topic", "2020-01-04T00:00:00Z");
    git(&repo, &["checkout", "-q", "-b", "clash", &root_sha]);
    commit_file(&repo, "README", "three\n", "clash", "2020-01-05T00:00:00Z");
    git(&repo, &["checkout", "-q", "main"]);

    let before = git(
        &repo,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let plan = root.join("plan.org");
    let ran = triage(&repo, &plan);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "stdout {}\nstderr {}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        git(
            &repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        before
    );
    let text = fs::read_to_string(&plan).unwrap();
    assert!(text.contains("#+stacks: 2"));
    assert!(text.contains("#+order: 3"));
    let lines = order_lines(&text);
    let base = lines
        .iter()
        .position(|l| l.contains("land") && l.contains("refs/heads/base"))
        .expect(&text);
    let topic = lines
        .iter()
        .position(|l| l.contains("land") && l.contains("refs/heads/topic"))
        .expect(&text);
    assert!(base < topic, "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("rebase") && l.contains("refs/heads/clash")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("refs/heads/gone")));
    assert!(text.contains("refs/heads/gone"));
    let stacks = stacks_section(&text);
    let base_line = stacks
        .lines()
        .find(|l| l.contains("refs/heads/base"))
        .unwrap();
    let topic_line = stacks
        .lines()
        .find(|l| l.contains("refs/heads/topic"))
        .unwrap();
    assert!(base_line.starts_with("- "), "{base_line}");
    assert!(topic_line.starts_with("  - "), "{topic_line}");
    let stdout = String::from_utf8_lossy(&ran.stdout);
    assert!(stdout.contains("stacks 2"));
    assert!(stdout.contains("order 3"));
    assert!(stdout.contains("drop "));

    let again = root.join("plan-again.org");
    let ran = triage(&repo, &again);
    assert_eq!(ran.status.code(), Some(0));
    assert_eq!(fs::read(&plan).unwrap(), fs::read(&again).unwrap());
}

#[test]
fn triage_reads_remote_heads_without_writing_the_checkout() {
    let root = tmp();
    let src = root.join("src");
    init(&src);
    commit_file(&src, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    git(&src, &["checkout", "-q", "-b", "feature"]);
    commit_file(&src, "f.txt", "f\n", "feature", "2020-01-02T00:00:00Z");
    git(&src, &["checkout", "-q", "main"]);
    let dst = root.join("dst");
    git(
        &root,
        &["clone", "-q", src.to_str().unwrap(), dst.to_str().unwrap()],
    );
    let before = git(&dst, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let plan = root.join("plan.org");
    let ran = triage(&dst, &plan);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "stdout {}\nstderr {}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        git(&dst, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        before
    );
    let text = fs::read_to_string(&plan).unwrap();
    let lines = order_lines(&text);
    assert!(
        lines
            .iter()
            .any(|l| { l.contains("land") && l.contains("refs/remotes/origin/feature") }),
        "{text}"
    );
}

#[test]
fn stamp_then_track_round_trips_the_change_id() {
    let root = tmp();
    let repo = root.join("repo");
    init(&repo);
    commit_file(&repo, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    let bin = env!("CARGO_BIN_EXE_kitline");
    let stamped = Command::new(bin)
        .args(["stamp", "-m", "hello"])
        .output()
        .unwrap();
    assert_eq!(
        stamped.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&stamped.stderr)
    );
    let message = String::from_utf8(stamped.stdout.clone()).unwrap();
    assert!(message.contains("hello"), "{message}");
    let id = message
        .lines()
        .find_map(|l| l.strip_prefix("Kitline-Change: "))
        .expect(&message)
        .trim()
        .to_string();
    assert_eq!(id.len(), 32, "{id}");
    assert!(id.chars().all(|c| ('k'..='z').contains(&c)), "{id}");
    let msg_path = root.join("msg.txt");
    fs::write(&msg_path, &message).unwrap();
    let again = Command::new(bin)
        .args(["stamp", "-F", msg_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(again.stdout, stamped.stdout);
    git(
        &repo,
        &[
            "commit",
            "--allow-empty",
            "-q",
            "-F",
            msg_path.to_str().unwrap(),
        ],
    );
    let tracked = Command::new(bin)
        .args(["track", "HEAD", "--repo", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        tracked.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&tracked.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&tracked.stdout).trim(), id);
    git(&repo, &["commit", "--allow-empty", "-qm", "plain"]);
    let missing = Command::new(bin)
        .args(["track", "HEAD", "--repo", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(missing.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no Kitline-Change"));
}

#[test]
fn help_lists_the_dogfood_verbs_and_hides_broken_flags() {
    let bin = env!("CARGO_BIN_EXE_kitline");
    let help = Command::new(bin).arg("--help").output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("triage"));
    assert!(text.contains("stamp"));
    assert!(text.contains("track"));
    assert!(!text.contains("--tickets"));
    assert!(!text.contains("--online"));
    let plan = Command::new(bin)
        .args(["--workspace", "--help"])
        .output()
        .unwrap();
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(shown.contains("--workspace"), "{shown}");
    assert!(!shown.contains("--tickets"), "{shown}");
    assert!(!shown.contains("--online"), "{shown}");
}
