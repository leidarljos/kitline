use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
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

fn init_at(path: &Path) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/triage/init.sh");
    let out = Command::new("sh").arg(&script).arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "fixture\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fixture(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    init_at(&repo);
    repo
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
    std::str::from_utf8(&raw[start..end])
        .map(str::to_string)
        .unwrap_or_default()
}

fn cell_text(cell: &str) -> String {
    let raw = unescape_cell(cell);
    match String::from_utf8(raw) {
        Ok(text) => text,
        Err(err) => escape_cell(&err.into_bytes()),
    }
}

fn unescape_cell(cell: &str) -> Vec<u8> {
    let b = cell.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        assert!(i + 1 < b.len(), "truncated escape");
        i += 1;
        match b[i] {
            b'\\' => out.push(b'\\'),
            b'|' => out.push(b'|'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'0'..=b'7' => {
                assert!(i + 2 < b.len(), "truncated octal");
                let digits = &b[i..i + 3];
                assert!(
                    digits.iter().all(|d| (b'0'..=b'7').contains(d)),
                    "bad octal"
                );
                let val = u16::from(digits[0] - b'0') * 64
                    + u16::from(digits[1] - b'0') * 8
                    + u16::from(digits[2] - b'0');
                assert!(val <= 255, "octal out of range");
                out.push(val as u8);
                i += 2;
            }
            other => panic!("unknown escape {other}"),
        }
        i += 1;
    }
    out
}

fn escape_cell(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let end = raw.len().saturating_sub(1);
    let mut i = 0;
    while i < raw.len() {
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

fn git_raw(dir: &Path, args: &[&[u8]]) -> Vec<u8> {
    let os: Vec<&OsStr> = args.iter().map(|a| OsStr::from_bytes(a)).collect();
    let out = Command::new("git")
        .args(os)
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
    out.stdout
}

fn move_drop(repo: &Path, name: &[u8]) {
    git_raw(repo, &[b"checkout", b"-q", b"--detach", name]);
    fs::write(repo.join("moved.txt"), b"moved\n").unwrap();
    git_raw(repo, &[b"add", b"moved.txt"]);
    git_raw(repo, &[b"commit", b"-qm", b"move the drop head"]);
    let tip = fs::read_to_string(repo.join(".git/HEAD")).unwrap();
    let tip = tip.trim().as_bytes().to_vec();
    assert_eq!(tip.len(), 40, "{}", String::from_utf8_lossy(&tip));
    git_raw(repo, &[b"update-ref", name, &tip]);
}

fn verify_names(plan: &Path, needle: &str) {
    let again = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", plan.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(again.status.code(), Some(0));
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains(needle), "{err}");
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
                let value = value.strip_prefix(' ').unwrap_or(value);
                let value = if key == "repo" || key == "trunk" {
                    cell_text(value)
                } else {
                    value.to_string()
                };
                header.insert(key.to_string(), value);
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
        let got: Vec<String> = got.iter().map(|c| cell_text(c)).collect();
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
        .current_dir(out.parent().unwrap())
        .output()
        .unwrap()
}

/// grade/<grade>/<bucket>[/slug], plus the named heads this round adds.
fn label(short: &str) -> (String, String) {
    if short == "v1" {
        return ("patch-equal".into(), "drop".into());
    }
    if short == "HEAD" {
        return ("open".into(), "rest".into());
    }
    if short == "--output=x" || short == "grade/open/dates" {
        return ("open".into(), "rest".into());
    }
    if short == "grade/open/space" || short == "grade/open/lead" {
        return ("open".into(), "rebase".into());
    }
    if short.ends_with('\u{00a0}') {
        return ("ancestor".into(), "drop".into());
    }
    if short.chars().any(|c| c == '\u{00a0}') {
        return ("open".into(), "rest".into());
    }
    if short.contains('|') {
        return ("open".into(), "rest".into());
    }
    if short.contains('\\') {
        return ("ancestor".into(), "drop".into());
    }
    let mut parts = short.split('/');
    assert_eq!(parts.next(), Some("grade"), "{short}");
    let grade = parts.next().unwrap_or("").to_string();
    let bucket = parts.next().unwrap_or("").to_string();
    assert!(!grade.is_empty() && !bucket.is_empty(), "{short}");
    (grade, bucket)
}

fn bucket_title(bucket: &str) -> &'static str {
    match bucket {
        "keep" => "KEEP",
        "drop" => "DROP",
        "rebase" => "REBASE",
        "rest" => "REST",
        "unsorted" => "UNSORTED",
        other => panic!("bucket {other}"),
    }
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

    let cwd = std::env::current_dir().unwrap();
    for path in [repo.join("x"), root.join("x"), cwd.join("x")] {
        assert!(!path.exists(), "created {}", path.display());
    }

    let mut seen = BTreeMap::new();
    for (branch, row) in &plan.rows {
        let section = plan.section.get(branch).unwrap();
        let short = branch
            .strip_prefix("refs/heads/")
            .unwrap_or_else(|| panic!("short name {branch}"));
        if short == "main" {
            assert_eq!(section, "KEEP");
            assert_eq!(row.get("grade").map(String::as_str), Some("keep"));
            assert_eq!(row.get("ahead").map(String::as_str), Some("0"));
            assert_eq!(row.get("behind").map(String::as_str), Some("0"));
            seen.insert(branch.clone(), section.clone());
            continue;
        }
        let (grade, bucket) = label(short);
        assert_eq!(
            row.get("grade").map(String::as_str),
            Some(grade.as_str()),
            "{branch}"
        );
        assert_eq!(section, bucket_title(&bucket), "{branch}");
        if grade != "unrelated" {
            assert!(!row.get("base").unwrap().is_empty(), "{branch}");
        }
        assert!(!row.get("author_old").unwrap().is_empty(), "{branch}");
        assert!(!row.get("author_new").unwrap().is_empty(), "{branch}");
        row.get("ahead").unwrap().parse::<u64>().unwrap();
        row.get("behind").unwrap().parse::<u64>().unwrap();
        seen.insert(branch.clone(), section.clone());
    }

    let noop = plan.rows.get("refs/heads/grade/no-op/drop").unwrap();
    assert!(noop.get("author_old").unwrap().starts_with("2020-01-07"));
    assert!(noop.get("author_new").unwrap().starts_with("2020-01-08"));
    assert_eq!(noop.get("check").map(String::as_str), Some("no-op"));
    assert!(!noop.get("evidence").unwrap().is_empty());

    let ancestor = plan.rows.get("refs/heads/grade/ancestor/drop").unwrap();
    assert_eq!(ancestor.get("ahead").map(String::as_str), Some("0"));
    assert_eq!(ancestor.get("check").map(String::as_str), Some("ancestor"));
    assert!(ancestor.get("evidence").unwrap().contains("is-ancestor"));

    let equal = plan.rows.get("refs/heads/grade/patch-equal/drop").unwrap();
    assert_eq!(equal.get("check").map(String::as_str), Some("patch-equal"));
    assert!(!equal.get("evidence").unwrap().is_empty());
    let reverted = plan.rows.get("refs/heads/grade/patch-equal/rest").unwrap();
    assert_eq!(reverted.get("check").map(String::as_str), Some(""));
    assert_eq!(
        reverted.get("reason").map(String::as_str),
        Some("merged then reverted or modified upstream")
    );

    let partial = plan.rows.get("refs/heads/grade/partial/rebase").unwrap();
    assert!(!partial.get("dropped").unwrap().is_empty());
    let conflict = plan.rows.get("refs/heads/grade/open/rebase").unwrap();
    assert_eq!(conflict.get("applies").map(String::as_str), Some("no"));
    assert!(conflict.get("conflicts").unwrap().contains("conflict.txt"));
    let applies = plan.rows.get("refs/heads/grade/open/rest").unwrap();
    assert_eq!(applies.get("applies").map(String::as_str), Some("yes"));
    assert!(applies.get("behind").unwrap().parse::<u64>().unwrap() > 0);

    for row in plan.rows.values() {
        if plan.section.get(row.get("branch").unwrap()).unwrap() == "DROP" {
            assert_eq!(row.get("grade"), row.get("check"));
            assert!(!row.get("evidence").unwrap().is_empty());
        }
    }

    let v1 = plan.rows.get("refs/heads/v1").unwrap();
    assert_eq!(v1.get("check").map(String::as_str), Some("patch-equal"));
    assert_ne!(
        v1.get("sha").map(String::as_str),
        Some(git(&repo, &["rev-parse", "refs/tags/v1"]).as_str()),
        "v1 was graded as the tag"
    );
    let span = plan.rows.get("refs/heads/grade/open/dates").unwrap();
    assert_eq!(
        span.get("author_old").map(String::as_str),
        Some("2020-01-02T00:00:00Z")
    );
    assert_eq!(
        span.get("author_new").map(String::as_str),
        Some("2020-01-01T23:00:00-05:00")
    );
    let space = plan.rows.get("refs/heads/grade/open/space").unwrap();
    assert_eq!(space.get("applies").map(String::as_str), Some("no"));
    assert!(
        space.get("conflicts").unwrap().contains("my file.txt"),
        "{space:?}"
    );
    let squashed = plan.rows.get("refs/heads/grade/squash-equal/rest").unwrap();
    assert_eq!(
        plan.section
            .get("refs/heads/grade/squash-equal/rest")
            .map(String::as_str),
        Some("REST")
    );
    assert_eq!(
        squashed.get("grade").map(String::as_str),
        Some("squash-equal")
    );
    let lead = plan.rows.get("refs/heads/grade/open/lead").unwrap();
    assert_eq!(
        plan.section
            .get("refs/heads/grade/open/lead")
            .map(String::as_str),
        Some("REBASE")
    );
    assert_eq!(lead.get("grade").map(String::as_str), Some("open"));
    assert_eq!(lead.get("conflicts").map(String::as_str), Some(" foo.txt"));
    let orphan = plan
        .rows
        .get("refs/heads/grade/unrelated/unsorted")
        .unwrap();
    assert_eq!(orphan.get("reason").map(String::as_str), Some("unrelated"));
    assert!(plan.rows.keys().any(|k| k.contains('\u{00a0}')));
    assert!(plan.rows.contains_key("refs/heads/--output=x"));
    assert!(plan.rows.contains_key("refs/heads/HEAD"));
    let trail = format!("refs/heads/trail{}", '\u{00a0}');
    assert_eq!(
        plan.section.get(&trail).map(String::as_str),
        Some("DROP"),
        "{trail:?}"
    );
    assert_eq!(
        plan.rows
            .get(&trail)
            .unwrap()
            .get("grade")
            .map(String::as_str),
        Some("ancestor")
    );
    assert_eq!(
        plan.section.get("refs/heads/foo|bar").map(String::as_str),
        Some("REST")
    );
    assert_eq!(
        plan.rows
            .get("refs/heads/foo|bar")
            .unwrap()
            .get("grade")
            .map(String::as_str),
        Some("open")
    );
    let bad = "refs/heads/bad\\377name";
    assert_eq!(plan.section.get(bad).map(String::as_str), Some("DROP"));
    assert_eq!(
        plan.rows.get(bad).unwrap().get("grade").map(String::as_str),
        Some("ancestor")
    );
    assert_eq!(unescape_cell(bad), b"refs/heads/bad\xffname");

    let listed = git_raw(
        &repo,
        &[b"for-each-ref", b"--format=%(refname)", b"refs/heads"],
    );
    let mut names: Vec<Vec<u8>> = listed
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| l.to_vec())
        .collect();
    names.sort();
    let mut planned: Vec<Vec<u8>> = plan.rows.keys().map(|k| unescape_cell(k)).collect();
    planned.sort();
    assert_eq!(planned, names);
    for key in ["keep", "drop", "rebase", "rest", "unsorted"] {
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

    let text = fs::read_to_string(&plan_path).unwrap();
    let plan = parse(&text);
    assert_eq!(
        plan.section.get("refs/heads/v1").map(String::as_str),
        Some("DROP")
    );
    let trail = format!("refs/heads/trail{}", '\u{00a0}');
    assert_eq!(plan.section.get(&trail).map(String::as_str), Some("DROP"));
    let bad = "refs/heads/bad\\377name";
    assert_eq!(plan.section.get(bad).map(String::as_str), Some("DROP"));
    move_drop(&repo, trail.as_bytes());
    verify_names(&plan_path, &trail);
    let trail_sha = plan
        .rows
        .get(&trail)
        .unwrap()
        .get("sha")
        .unwrap()
        .as_bytes()
        .to_vec();
    git_raw(&repo, &[b"update-ref", trail.as_bytes(), &trail_sha]);
    git_raw(&repo, &[b"checkout", b"-qf", b"main"]);
    move_drop(&repo, b"refs/heads/bad\xffname");
    verify_names(&plan_path, bad);
    let bad_sha = plan
        .rows
        .get(bad)
        .unwrap()
        .get("sha")
        .unwrap()
        .as_bytes()
        .to_vec();
    git_raw(&repo, &[b"update-ref", b"refs/heads/bad\xffname", &bad_sha]);
    git_raw(&repo, &[b"checkout", b"-qf", b"main"]);
    let tag = git(&repo, &["rev-parse", "refs/tags/v1"]);
    // checkout of the full ref detaches, so point the branch at the new commit.
    git(&repo, &["checkout", "-q", "--detach", "refs/heads/v1"]);
    fs::write(repo.join("moved.txt"), "moved\n").unwrap();
    git(&repo, &["add", "moved.txt"]);
    git(&repo, &["commit", "-qm", "move the drop head"]);
    let tip = fs::read_to_string(repo.join(".git/HEAD")).unwrap();
    let tip = tip.trim();
    assert_eq!(tip.len(), 40, "{tip}");
    git(&repo, &["update-ref", "refs/heads/v1", tip]);
    assert_eq!(git(&repo, &["rev-parse", "refs/tags/v1"]), tag);
    assert_ne!(git(&repo, &["rev-parse", "refs/heads/v1"]), tag);
    let again = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", plan_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(again.status.code(), Some(0));
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("refs/heads/v1"), "{err}");

    // #+trunk is main plus U+00A0, a different head from main.
    let nbsp_trunk = format!("main{}", '\u{00a0}');
    let trunk_repo = root.join("trunk-repo");
    init_at(&trunk_repo);
    let main_sha = git(&trunk_repo, &["rev-parse", "refs/heads/main"]);
    let trunk_ref = format!("refs/heads/{nbsp_trunk}");
    git(&trunk_repo, &["update-ref", &trunk_ref, &main_sha]);
    let trunk_plan = root.join("trunk-plan.org");
    let ran = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args([
            "triage",
            trunk_repo.to_str().unwrap(),
            "--trunk",
            &nbsp_trunk,
            "--keep",
            "grade/keep/*",
            "--out",
            trunk_plan.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let trunk_text = fs::read_to_string(&trunk_plan).unwrap();
    assert_eq!(
        parse(&trunk_text).header.get("trunk").map(String::as_str),
        Some(nbsp_trunk.as_str())
    );
    let tree = git(&trunk_repo, &["rev-parse", &format!("{main_sha}^{{tree}}")]);
    let orphan = git(&trunk_repo, &["commit-tree", &tree, "-m", "orphan"]);
    git(&trunk_repo, &["update-ref", &trunk_ref, &orphan]);
    assert_eq!(
        git(&trunk_repo, &["rev-parse", "refs/heads/main"]),
        main_sha
    );
    let moved_trunk = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", trunk_plan.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(moved_trunk.status.code(), Some(0));
    let err = String::from_utf8_lossy(&moved_trunk.stderr);
    assert!(err.contains("drop does not hold"), "{err}");

    // #+repo ends in U+00A0. The directory without that byte stays put.
    let plain_dir = root.join("rdir");
    let named_dir = root.join(format!("rdir{}", '\u{00a0}'));
    init_at(&plain_dir);
    init_at(&named_dir);
    let plain_v1 = git(&plain_dir, &["rev-parse", "refs/heads/v1"]);
    let repo_plan = root.join("repo-plan.org");
    let ran = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args([
            "triage",
            named_dir.to_str().unwrap(),
            "--trunk",
            "main",
            "--keep",
            "grade/keep/*",
            "--out",
            repo_plan.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let repo_text = fs::read_to_string(&repo_plan).unwrap();
    let recorded = parse(&repo_text).header.get("repo").unwrap().clone();
    assert_eq!(
        recorded.as_str(),
        named_dir.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(recorded.ends_with('\u{00a0}'), "{recorded}");
    move_drop(&named_dir, b"refs/heads/v1");
    assert_eq!(git(&plain_dir, &["rev-parse", "refs/heads/v1"]), plain_v1);
    let moved_repo = Command::new(env!("CARGO_BIN_EXE_kitline"))
        .args(["triage", "verify", repo_plan.to_str().unwrap()])
        .output()
        .unwrap();
    assert_ne!(moved_repo.status.code(), Some(0));
    let err = String::from_utf8_lossy(&moved_repo.stderr);
    assert!(err.contains("refs/heads/v1"), "{err}");
}
