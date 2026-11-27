use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
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

fn section(text: &str, title: &str) -> String {
    let mut on = false;
    let mut out = String::new();
    let header = format!("* {title}");
    for line in text.lines() {
        if line == header {
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

fn sh_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        if line == "#+begin_src sh" {
            cur = Some(String::new());
            continue;
        }
        if line == "#+end_src" {
            if let Some(body) = cur.take() {
                blocks.push(body);
            }
            continue;
        }
        if let Some(body) = cur.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    blocks
}

fn reverse_order(text: &str) -> String {
    let marker = "* ORDER\n";
    let at = text.find(marker).expect("ORDER");
    let (head, rest) = text.split_at(at + marker.len());
    let (body, tail) = match rest.find("\n* ") {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let mut items: Vec<&str> = body.lines().filter(|line| !line.is_empty()).collect();
    assert!(items.len() >= 2, "need a parent and a child\n{text}");
    items.reverse();
    let mut out = String::from(head);
    out.push('\n');
    for item in items {
        out.push_str(item);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(tail);
    out
}

/// `refs/remotes/origin/main` is a head of its own. When local `main` exists
/// it is the only trunk, and a remote that is ahead of it is ordered.
#[test]
fn remote_tracking_trunk_is_not_a_second_trunk() {
    let root = tmp();
    let repo = root.join("repo");
    init(&repo);
    commit_file(&repo, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    commit_file(&repo, "README", "two\n", "local", "2020-01-02T00:00:00Z");
    let local = git(&repo, &["rev-parse", "HEAD"]);
    commit_file(&repo, "a.txt", "a\n", "ahead", "2020-01-03T00:00:00Z");
    commit_file(&repo, "b.txt", "b\n", "ahead-more", "2020-01-04T00:00:00Z");
    let remote = git(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(local, remote);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &remote]);
    git(&repo, &["reset", "-q", "--hard", &local]);

    let plan = root.join("plan.org");
    let ran = triage(&repo, &plan);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "stdout {}\nstderr {}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    let text = fs::read_to_string(&plan).unwrap();
    let keep = section(&text, "KEEP");
    let order = section(&text, "ORDER");
    assert!(keep.contains("refs/heads/main"), "{keep}");
    assert!(
        !keep.contains("refs/remotes/origin/main"),
        "origin/main was treated as trunk\n{keep}"
    );
    assert!(
        order.contains("refs/remotes/origin/main"),
        "origin/main was left out of ORDER\n{text}"
    );
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("order 1"),
        "{}",
        String::from_utf8_lossy(&ran.stdout)
    );

    git(&repo, &["checkout", "-q", "--detach", &remote]);
    git(&repo, &["branch", "-D", "main"]);
    let remote_plan = root.join("plan-remote.org");
    let ran = triage(&repo, &remote_plan);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let text = fs::read_to_string(&remote_plan).unwrap();
    let keep = section(&text, "KEEP");
    let order = section(&text, "ORDER");
    assert!(
        keep.contains("refs/remotes/origin/main") && keep.contains("trunk"),
        "remote trunk was not chosen\n{keep}"
    );
    assert!(
        !order.contains("refs/remotes/origin/main"),
        "the chosen trunk was ordered\n{order}"
    );
}

/// A depth-1 clone has missing history. triage must not write a plan.
#[test]
fn shallow_clone_does_not_plan() {
    let root = tmp();
    let full = root.join("full");
    init(&full);
    commit_file(&full, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    let root_sha = git(&full, &["rev-parse", "HEAD"]);
    git(&full, &["branch", "old", &root_sha]);
    commit_file(&full, "README", "two\n", "main", "2020-01-02T00:00:00Z");
    git(&full, &["checkout", "-q", "-b", "feature"]);
    commit_file(
        &full,
        "feature.txt",
        "f\n",
        "feature",
        "2020-01-03T00:00:00Z",
    );
    git(&full, &["checkout", "-q", "main"]);
    let shallow = root.join("shallow");
    let url = format!("file://{}", full.display());
    git(
        &root,
        &[
            "clone",
            "--depth",
            "1",
            "--no-single-branch",
            "--no-local",
            &url,
            shallow.to_str().unwrap(),
        ],
    );
    assert_eq!(
        git(&shallow, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );

    let plan = root.join("plan.org");
    let ran = triage(&shallow, &plan);
    let err = String::from_utf8_lossy(&ran.stderr);
    let out = String::from_utf8_lossy(&ran.stdout);
    assert_ne!(ran.status.code(), Some(0), "stdout {out}\nstderr {err}");
    let lines: Vec<&str> = err.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1, "{err}");
    assert!(lines[0].contains("git fetch --unshallow"), "{err}");
    assert!(!plan.exists(), "wrote a plan from a shallow clone");
    assert!(!out.contains("order "), "{out}");
    assert!(!out.contains("wrote "), "{out}");
}

/// The quickstart block commits the printed trailer, then track prints it.
#[test]
fn quickstart_commits_the_stamp_before_track() {
    let qs = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("QUICKSTART.org"))
        .unwrap();
    let block = sh_blocks(&qs)
        .into_iter()
        .find(|body| body.contains("kitline stamp") && body.contains("kitline track HEAD"))
        .expect("quickstart stamp block");
    let root = tmp();
    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_kitline"), bin_dir.join("kitline")).unwrap();
    let script = root.join("quickstart.sh");
    fs::write(&script, &block).unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let ran = Command::new("sh")
        .arg("-e")
        .arg(&script)
        .current_dir(&root)
        .env("PATH", path)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&ran.stdout);
    let err = String::from_utf8_lossy(&ran.stderr);
    assert!(
        ran.status.success(),
        "quickstart failed\n{block}\nstdout {out}\nstderr {err}"
    );
    assert!(
        out.lines()
            .any(|line| { line.len() == 32 && line.chars().all(|c| ('k'..='z').contains(&c)) }),
        "track did not print the stamp id\n{out}"
    );
}

/// verify re-reads ORDER. A parent listed after its child does not hold.
#[test]
fn verify_rejects_a_parent_after_its_child() {
    let root = tmp();
    let repo = root.join("repo");
    init(&repo);
    commit_file(&repo, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    git(&repo, &["checkout", "-q", "-b", "base"]);
    commit_file(&repo, "a.txt", "a\n", "base", "2020-01-02T00:00:00Z");
    git(&repo, &["checkout", "-q", "-b", "child"]);
    commit_file(&repo, "b.txt", "b\n", "child", "2020-01-03T00:00:00Z");
    git(&repo, &["checkout", "-q", "main"]);

    let plan = root.join("plan.org");
    let ran = triage(&repo, &plan);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let text = fs::read_to_string(&plan).unwrap();
    let lines = order_lines(&text);
    let base = lines
        .iter()
        .position(|line| line.contains("refs/heads/base"))
        .expect(&text);
    let child = lines
        .iter()
        .position(|line| line.contains("refs/heads/child"))
        .expect(&text);
    assert!(base < child, "{lines:?}");
    let fresh = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", plan.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        fresh.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );

    let bad = root.join("reversed.org");
    fs::write(&bad, reverse_order(&text)).unwrap();
    let reversed = fs::read_to_string(&bad).unwrap();
    let lines = order_lines(&reversed);
    assert!(
        lines
            .iter()
            .position(|line| line.contains("refs/heads/child"))
            .unwrap()
            < lines
                .iter()
                .position(|line| line.contains("refs/heads/base"))
                .unwrap(),
        "{lines:?}"
    );
    let verify = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", bad.to_str().unwrap()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&verify.stderr);
    assert_ne!(verify.status.code(), Some(0), "{err}");
    assert!(
        err.contains("order puts") && err.contains("refs/heads/base") && err.contains("after"),
        "{err}"
    );
    assert!(err.contains("refs/heads/child"), "{err}");
}

/// A non-UTF-8 argument is an error. `std::env::args` panics instead.
#[test]
fn non_utf8_argument_is_an_error() {
    let root = tmp();
    let repo = root.join("repo");
    init(&repo);
    commit_file(&repo, "README", "one\n", "root", "2020-01-01T00:00:00Z");
    let bin = env!("CARGO_BIN_EXE_kitline");
    let bad = OsStr::from_bytes(b"refs/heads/new\xff");

    let tracked = Command::new(bin)
        .arg("track")
        .arg("--repo")
        .arg(&repo)
        .arg(bad)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&tracked.stderr);
    assert_eq!(tracked.status.code(), Some(1), "{err}");
    assert!(err.contains("utf-8"), "{err}");
    assert!(!err.contains("panicked"), "{err}");

    let triaged = Command::new(bin)
        .arg("triage")
        .arg(&repo)
        .arg("--trunk")
        .arg(bad)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&triaged.stderr);
    assert_eq!(triaged.status.code(), Some(1), "{err}");
    assert!(err.contains("utf-8"), "{err}");
    assert!(!err.contains("panicked"), "{err}");

    let first = Command::new(bin).arg(bad).output().unwrap();
    let err = String::from_utf8_lossy(&first.stderr);
    assert_eq!(first.status.code(), Some(1), "{err}");
    assert!(err.contains("utf-8"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
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
