use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Out {
    code: i32,
    report: String,
    script: String,
    script_path: PathBuf,
}

fn tmp() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("kitline-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init(root: &Path, name: &str) -> PathBuf {
    let p = root.join(name);
    fs::create_dir_all(&p).unwrap();
    git(&p, &["init", "-q", "-b", "develop"]);
    git(&p, &["config", "user.email", "rgoswami@ieee.org"]);
    git(&p, &["config", "user.name", "Rohit Goswami"]);
    git(&p, &["config", "commit.gpgsign", "false"]);
    git(&p, &["commit", "--allow-empty", "-qm", "root"]);
    p
}

fn rev(dir: &Path, spec: &str) -> String {
    git(dir, &["rev-parse", spec])
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", msg]);
}

fn branch_new(dir: &Path, name: &str) {
    git(dir, &["checkout", "-q", "-b", name]);
}

fn ahead(dir: &Path) {
    git(dir, &["commit", "--allow-empty", "-qm", "ahead"]);
}

fn checkout(dir: &Path, name: &str) {
    git(dir, &["checkout", "-q", name]);
}

fn grow(dir: &Path, n: u32, msg: &str) {
    let mut parent = rev(dir, "HEAD");
    let tree = rev(dir, "HEAD^{tree}");
    for i in 0..n {
        let m = format!("{msg}-{i}");
        parent = git(dir, &["commit-tree", &tree, "-p", &parent, "-m", &m]);
    }
    git(dir, &["update-ref", "HEAD", &parent]);
}

fn same_tree(dir: &Path) -> (String, String) {
    let new = rev(dir, "HEAD");
    let tree = rev(dir, "HEAD^{tree}");
    let parent = rev(dir, "HEAD^");
    let old = git(dir, &["commit-tree", &tree, "-p", &parent, "-m", "old"]);
    (old, new)
}

fn bundle_on_tip(dir: &Path, out: &Path) -> String {
    let tip = rev(dir, "HEAD");
    let tree = rev(dir, "HEAD^{tree}");
    let extra = git(dir, &["commit-tree", &tree, "-p", &tip, "-m", "extra"]);
    git(dir, &["update-ref", "refs/tmp/extra", &extra]);
    git(
        dir,
        &[
            "bundle",
            "create",
            &out.to_string_lossy(),
            "refs/tmp/extra",
            &format!("^{tip}"),
        ],
    );
    tip
}

fn plan(root: &Path, ws: &str, extra: &[&str]) -> Out {
    let ws_path = root.join("workspace.toml");
    fs::write(&ws_path, ws).unwrap();
    let report = root.join("out.org");
    let script = root.join("out.sh");
    let bin = env!("CARGO_BIN_EXE_kitline");
    let status = Command::new(bin)
        .arg("--workspace")
        .arg(&ws_path)
        .arg("--report")
        .arg(&report)
        .arg("--script")
        .arg(&script)
        .args(extra)
        .status()
        .unwrap();
    Out {
        code: status.code().unwrap_or(1),
        report: fs::read_to_string(&report).unwrap_or_default(),
        script: fs::read_to_string(&script).unwrap_or_default(),
        script_path: script,
    }
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "missing {needle}\n{text}");
}

fn not_has(text: &str, needle: &str) {
    assert!(!text.contains(needle), "unexpected {needle}\n{text}");
}

fn cargo_pkg(name: &str, deps: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n{deps}")
}

#[test]
fn step_01_kit_lease_tree() {
    let root = tmp();
    let ttech = init(&root, "ttech");
    branch_new(&ttech, "land");
    git(&ttech, &["commit", "--allow-empty", "-qm", "land"]);
    let bundle = root.join("ttech.bundle");
    let tip = bundle_on_tip(&ttech, &bundle);

    let ridley = init(&root, "ridley");
    write_file(&ridley, "src/lib.rs", "pub fn core() {}\n");
    commit_all(&ridley, "core");
    let (old, new) = same_tree(&ridley);
    branch_new(&ridley, "main");

    let desk = init(&root, "desktop");
    write_file(
        &desk,
        "Cargo.toml",
        &cargo_pkg(
            "desktop",
            &format!("[dependencies]\nridley-core = {{ git = \"https://github.com/ttech/ridley\", rev = \"{old}\" }}\n"),
        ),
    );
    commit_all(&desk, "pin");
    branch_new(&desk, "desk");
    ahead(&desk);

    let bad = init(&root, "badtree");
    write_file(&bad, "f.txt", "hello\n");
    commit_all(&bad, "f");
    branch_new(&bad, "land");
    let actual = rev(&bad, "HEAD^{tree}");
    let bad_bundle = root.join("bad.bundle");
    bundle_on_tip(&bad, &bad_bundle);
    let empty = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    let ws = format!(
        r#"
ci = "cargo test"

[[node]]
id = "ttech-land"
repo = "ttech"
path = "ttech"
base = "develop"
branch = "land"
lease_matches = "head"
source = "bundle"
bundle = "ttech.bundle"
open_pr = false

[[node]]
id = "ridley"
repo = "ridley"
path = "ridley"
base = "develop"
branch = "main"

[[node]]
id = "desktop"
repo = "desktop"
path = "desktop"
base = "develop"
branch = "desk"

[[node]]
id = "bad-tree"
repo = "badtree"
path = "badtree"
base = "develop"
branch = "land"
lease_matches = "head"
source = "bundle"
bundle = "bad.bundle"
tree = "{empty}"
"#
    );
    let out = plan(&root, &ws, &[]);
    assert_eq!(out.code, 0, "{}", out.report);
    has(&out.script, "git push origin HEAD:land");
    not_has(&out.script, "--force");
    not_has(&out.script, "--no-verify");
    not_has(&out.script, "gh pr create");
    has(&out.script, &new);
    has(&out.report, &tip);
    has(&out.report, "edge: ridley -> desktop cargo");
    assert_eq!(
        out.report.matches("edge: ridley -> desktop cargo").count(),
        1
    );
    has(&out.report, &actual);
    has(&out.report, empty);
    has(&out.report, "refused:");
    has(&out.report, "bad-tree");

    let other = init(&root, "other");
    write_file(&other, "z", "z\n");
    commit_all(&other, "z");
    let missing = rev(&other, "HEAD");
    write_file(&other, "z", "zz\n");
    commit_all(&other, "zz");
    let bad_b = root.join("missing.bundle");
    git(
        &other,
        &[
            "bundle",
            "create",
            &bad_b.to_string_lossy(),
            "HEAD",
            &format!("^{missing}"),
        ],
    );
    let miss = plan(
        &root,
        r#"
[[node]]
id = "anneal-179"
repo = "ttech"
path = "ttech"
base = "develop"
branch = "land"
lease_matches = "head"
source = "bundle"
bundle = "missing.bundle"
"#,
        &[],
    );
    assert_eq!(miss.code, 1, "{}", miss.report);
    has(&miss.report, &missing);
    assert!(!miss.script_path.exists());

    let open = plan(
        &root,
        r#"
[[node]]
id = "ttech-pr"
repo = "ttech"
path = "ttech"
base = "develop"
branch = "land"
lease_matches = "head"
source = "bundle"
bundle = "ttech.bundle"
open_pr = true
"#,
        &[],
    );
    assert_eq!(open.code, 0, "{}", open.report);
    let list = open.script.find("gh pr list").unwrap();
    let create = open.script.find("gh pr create").unwrap();
    assert!(list < create);
    not_has(&open.script, "gh pr merge");
}

