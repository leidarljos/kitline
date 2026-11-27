use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("kitline-triage-test-{}-{n}", std::process::id()));
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

fn fixture(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/triage/init.sh");
    let out = Command::new("sh").arg(&script).arg(&repo).output().unwrap();
    assert!(
        out.status.success(),
        "fixture\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    repo
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
    !cell.is_empty() && cell.chars().all(|c| c == '-' || c == '+')
}

struct Plan {
    header: BTreeMap<String, String>,
    rows: BTreeMap<String, BTreeMap<String, String>>,
    section: BTreeMap<String, String>,
}

fn parse(text: &str) -> Plan {
    let mut header = BTreeMap::new();
    let mut rows = BTreeMap::new();
    let mut section_of = BTreeMap::new();
    let mut section = String::new();
    let mut cols: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#+") {
            if let Some((key, value)) = rest.split_once(':') {
                header.insert(key.trim().to_string(), value.trim().to_string());
            }
            continue;
        }
        if let Some(title) = line.strip_prefix("* ") {
            section = title.trim().to_string();
            cols.clear();
            continue;
        }
        if section.is_empty() || !line.starts_with('|') {
            continue;
        }
        let got = cells(line);
        if got.is_empty() || got.iter().all(|c| dash(c)) {
            continue;
        }
        if cols.is_empty() {
            cols = got;
            continue;
        }
        let mut map = BTreeMap::new();
        for (i, cell) in got.iter().enumerate() {
            if let Some(name) = cols.get(i) {
                map.insert(name.clone(), cell.clone());
            }
        }
        let branch = map.get("branch").cloned().unwrap_or_default();
        assert!(!branch.is_empty(), "row without branch\n{text}");
        assert!(section_of.insert(branch.clone(), section.clone()).is_none());
        rows.insert(branch, map);
    }
    Plan {
        header,
        rows,
        section: section_of,
    }
}

fn triage(repo: &Path, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args([
            "triage",
            repo.to_str().unwrap(),
            "--trunk",
            "main",
            "--keep",
            "grade/keep/*",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

/// P21: every labelled head lands in the grade its name says.
#[test]
fn p21_fixture_grades_match_labels() {
    let root = tmp();
    let repo = fixture(&root);
    let before_refs = fs::read(repo.join(".git/packed-refs")).unwrap_or_default();
    let before_head = fs::read(repo.join(".git/HEAD")).unwrap();
    let before_index = fs::read(repo.join(".git/index")).unwrap();
    let before_names = git(
        &repo,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let plan_path = root.join("plan.org");
    let ran = triage(&repo, &plan_path);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "stdout {}\nstderr {}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(fs::read(repo.join(".git/HEAD")).unwrap(), before_head);
    assert_eq!(fs::read(repo.join(".git/index")).unwrap(), before_index);
    assert_eq!(
        fs::read(repo.join(".git/packed-refs")).unwrap_or_default(),
        before_refs
    );
    assert_eq!(
        git(
            &repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        before_names
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    let text = fs::read_to_string(&plan_path).unwrap();
    let plan = parse(&text);
    assert_eq!(
        plan.header.get("version").map(String::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        plan.header.get("repo").map(String::as_str),
        repo.canonicalize().unwrap().to_str()
    );
    assert_eq!(plan.header.get("trunk").map(String::as_str), Some("main"));
    let trunk_sha = plan.header.get("trunk_sha").unwrap();
    assert_eq!(trunk_sha.len(), 40);
    let snap = plan.header.get("snapshot").unwrap();
    assert!(snap.starts_with("sha256:"), "{snap}");
    assert_eq!(snap.len(), "sha256:".len() + 64);

    let mut seen = BTreeMap::new();
    for (branch, row) in &plan.rows {
        let section = plan.section.get(branch).unwrap();
        if branch == "main" {
            assert_eq!(section, "KEEP");
            assert_eq!(row.get("grade").map(String::as_str), Some("keep"));
            assert_eq!(row.get("ahead").map(String::as_str), Some("0"));
            assert_eq!(row.get("behind").map(String::as_str), Some("0"));
            seen.insert(branch.clone(), section.clone());
            continue;
        }
        let mut parts = branch.split('/');
        assert_eq!(parts.next(), Some("grade"), "{branch}");
        let grade = parts.next().unwrap();
        let bucket = parts.next().unwrap();
        assert!(parts.next().is_none(), "{branch}");
        assert_eq!(
            row.get("grade").map(String::as_str),
            Some(grade),
            "{branch}"
        );
        let want = match bucket {
            "keep" => "KEEP",
            "drop" => "DROP",
            "rebase" => "REBASE",
            "rest" => "REST",
            other => panic!("bucket {other}"),
        };
        assert_eq!(section, want, "{branch}");
        assert!(!row.get("base").unwrap().is_empty(), "{branch}");
        assert!(!row.get("author_old").unwrap().is_empty(), "{branch}");
        assert!(!row.get("author_new").unwrap().is_empty(), "{branch}");
        row.get("ahead").unwrap().parse::<u64>().unwrap();
        row.get("behind").unwrap().parse::<u64>().unwrap();
        seen.insert(branch.clone(), section.clone());
    }

    let noop = plan.rows.get("grade/no-op/drop").unwrap();
    assert!(noop.get("author_old").unwrap().starts_with("2020-01-07"));
    assert!(noop.get("author_new").unwrap().starts_with("2020-01-08"));
    assert_eq!(noop.get("check").map(String::as_str), Some("no-op"));
    assert!(!noop.get("evidence").unwrap().is_empty());

    let ancestor = plan.rows.get("grade/ancestor/drop").unwrap();
    assert_eq!(ancestor.get("ahead").map(String::as_str), Some("0"));
    assert_eq!(ancestor.get("check").map(String::as_str), Some("ancestor"));
    assert!(ancestor.get("evidence").unwrap().contains("is-ancestor"));

    let equal = plan.rows.get("grade/patch-equal/drop").unwrap();
    assert_eq!(equal.get("check").map(String::as_str), Some("patch-equal"));
    assert!(!equal.get("evidence").unwrap().is_empty());
    let reverted = plan.rows.get("grade/patch-equal/rest").unwrap();
    assert_eq!(reverted.get("check").map(String::as_str), Some(""));
    assert_eq!(
        reverted.get("reason").map(String::as_str),
        Some("merged then reverted or modified upstream")
    );

    let partial = plan.rows.get("grade/partial/rebase").unwrap();
    assert!(!partial.get("dropped").unwrap().is_empty());
    let conflict = plan.rows.get("grade/open/rebase").unwrap();
    assert_eq!(conflict.get("applies").map(String::as_str), Some("no"));
    assert!(conflict.get("conflicts").unwrap().contains("conflict.txt"));
    let applies = plan.rows.get("grade/open/rest").unwrap();
    assert_eq!(applies.get("applies").map(String::as_str), Some("yes"));
    assert!(applies.get("behind").unwrap().parse::<u64>().unwrap() > 0);

    for row in plan.rows.values() {
        if plan.section.get(row.get("branch").unwrap()).unwrap() == "DROP" {
            assert_eq!(row.get("grade"), row.get("check"));
            assert!(!row.get("evidence").unwrap().is_empty());
        }
    }

    let listed = git(
        &repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    );
    let mut names: Vec<&str> = listed.lines().collect();
    names.sort_unstable();
    let mut planned: Vec<&str> = plan.rows.keys().map(String::as_str).collect();
    planned.sort_unstable();
    assert_eq!(planned, names);
    for key in ["keep", "drop", "rebase", "rest"] {
        let n = seen
            .values()
            .filter(|s| s.eq_ignore_ascii_case(key))
            .count();
        assert_eq!(
            plan.header.get(key).map(String::as_str),
            Some(n.to_string()).as_deref()
        );
    }
}

/// P22: verify passes on a fresh plan and fails after a DROP head moves.
#[test]
fn p22_verify_fails_after_drop_moves() {
    let root = tmp();
    let repo = fixture(&root);
    let plan_path = root.join("plan.org");
    let ran = triage(&repo, &plan_path);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let verify = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", plan_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        verify.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );

    git(&repo, &["checkout", "-q", "grade/ancestor/drop"]);
    fs::write(repo.join("moved.txt"), "moved\n").unwrap();
    git(&repo, &["add", "moved.txt"]);
    git(&repo, &["commit", "-qm", "move the drop head"]);
    let again = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", plan_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(again.status.code(), Some(0));
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("grade/ancestor/drop"), "{err}");
}
