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

#[test]
fn step_02_stale_base_supersede() {
    let root = tmp();
    let eb = init(&root, "eb-local");
    branch_new(&eb, "fix/dev-locked-rpath-wrappers");
    grow(&eb, 3, "ahead");
    checkout(&eb, "develop");
    grow(&eb, 220, "behind");

    let cp = init(&root, "cpmdc");
    let tree = rev(&cp, "HEAD^{tree}");
    let orphan = git(&cp, &["commit-tree", &tree, "-m", "orphan"]);
    branch_new(&cp, "fix/mdi");

    let stack = init(&root, "stack");
    branch_new(&stack, "A");
    git(&stack, &["commit", "--allow-empty", "-qm", "A"]);
    branch_new(&stack, "B");
    git(&stack, &["commit", "--allow-empty", "-qm", "B"]);
    branch_new(&stack, "C");
    git(&stack, &["commit", "--allow-empty", "-qm", "C"]);

    let amsel = init(&root, "amsel");
    write_file(
        &amsel,
        "amsel-core/Cargo.toml",
        "[package]\nname = \"amsel-core\"\nversion = \"0.1.0\"\ntoken = \"base\"\n",
    );
    commit_all(&amsel, "base");
    branch_new(&amsel, "tica");
    write_file(
        &amsel,
        "amsel-core/Cargo.toml",
        "[package]\nname = \"amsel-core\"\nversion = \"0.1.0\"\ntoken = \"tica\"\n",
    );
    commit_all(&amsel, "tica");
    checkout(&amsel, "develop");
    branch_new(&amsel, "feat-70");
    write_file(
        &amsel,
        "amsel-core/Cargo.toml",
        "[package]\nname = \"amsel-core\"\nversion = \"0.1.0\"\ntoken = \"pr70\"\n",
    );
    commit_all(&amsel, "pr70");
    checkout(&amsel, "develop");
    branch_new(&amsel, "old-66");
    git(&amsel, &["commit", "--allow-empty", "-qm", "old"]);

    let anneal = init(&root, "anneal");
    branch_new(&anneal, "r251");
    git(&anneal, &["commit", "--allow-empty", "-qm", "251"]);
    checkout(&anneal, "develop");
    branch_new(&anneal, "r248");
    git(&anneal, &["commit", "--allow-empty", "-qm", "248"]);

    let rgpot = init(&root, "rgpot");
    branch_new(&rgpot, "open-505");
    git(&rgpot, &["commit", "--allow-empty", "-qm", "505"]);
    branch_new(&rgpot, "feat/md-session-nblib-r3");
    git(&rgpot, &["commit", "--allow-empty", "-qm", "r3"]);

    let ws = format!(
        r#"
ci = "cargo clippy --all-targets --all-features"

[[node]]
id = "eb-local-346"
repo = "eb-local"
path = "eb-local"
base = "develop"
branch = "fix/dev-locked-rpath-wrappers"
pr = 346
base_pr = 345
base_pr_state = "closed"

[[node]]
id = "cpmdc-191"
repo = "cpmdc"
path = "cpmdc"
base = "develop"
branch = "fix/mdi"
base_commit = "{orphan}"

[[node]]
id = "stack-a"
repo = "stack"
path = "stack"
base = "develop"
branch = "A"
pr = 405
pr_state = "open"

[[node]]
id = "stack-b"
repo = "stack"
path = "stack"
base = "A"
branch = "B"
pr_state = "open"
base_pr = 1
base_pr_state = "closed"

[[node]]
id = "stack-c"
repo = "stack"
path = "stack"
base = "B"
branch = "C"

[[node]]
id = "amsel-70"
repo = "amsel"
path = "amsel"
base = "develop"
branch = "feat-70"
pr = 70
supersedes = "amsel-66"

[[node]]
id = "amsel-66"
repo = "amsel"
path = "amsel"
base = "develop"
branch = "old-66"
pr = 66

[[node]]
id = "amsel-tica"
repo = "amsel"
path = "amsel"
base = "develop"
branch = "tica"

[[node]]
id = "anneal-251"
repo = "anneal"
path = "anneal"
base = "develop"
branch = "r251"
pr = 251
supersedes = "anneal-248"

[[node]]
id = "anneal-248"
repo = "anneal"
path = "anneal"
base = "develop"
branch = "r248"
pr = 248

[[node]]
id = "rgpot-505"
repo = "rgpot"
path = "rgpot"
base = "develop"
branch = "open-505"
pr = 505
pr_state = "open"
merge = "merge"

[[node]]
id = "rgpot-r3"
repo = "rgpot"
path = "rgpot"
base = "open-505"
branch = "feat/md-session-nblib-r3"
"#
    );
    let out = plan(&root, &ws, &[]);
    assert_eq!(out.code, 0, "{}", out.report);
    has(&out.report, "ahead: 3");
    has(&out.report, "behind: 220");
    has(&out.report, "base pr state: closed");
    has(&out.report, "not an ancestor");
    has(&out.report, "edge: stack-a -> stack-b ancestry");
    has(&out.report, "edge: stack-b -> stack-c ancestry");
    not_has(&out.report, "edge: stack-a -> stack-c ancestry");
    not_has(&out.report, "base pull request 1 is closed");
    has(&out.report, "amsel-core/Cargo.toml");
    has(&out.report, "superseded-by: amsel-70");
    has(&out.report, "supersedes: amsel-66");
    has(&out.report, "superseded-by: anneal-251");
    has(&out.report, "supersedes: anneal-248");
    has(&out.report, "merge: merge");
    has(
        &out.report,
        "open-505 because stacked pull request rgpot-r3 still bases on it",
    );
    has(&out.report, "cargo clippy --all-targets --all-features");
    has(&out.script, "HEAD:feat-70");
    has(&out.script, "HEAD:tica");
    not_has(&out.script, "fix/dev-locked-rpath-wrappers");
    not_has(&out.script, "HEAD:old-66");
    not_has(&out.script, "HEAD:r248");
    not_has(&out.script, "gh pr close");
    not_has(&out.script, "--delete");
    not_has(&out.script, "branch -d");
    not_has(&out.script, "branch -D");
}

#[test]
fn step_03_zig_and_cargo_pins() {
    let root = tmp();
    let zig = init(&root, "zig-common");
    write_file(&zig, "src/root.zig", "pub const x = 1;\n");
    commit_all(&zig, "lib");
    let bare = rev(&zig, "HEAD");
    git(&zig, &["tag", "v0.9.0"]);
    let dev_sha = bare.clone();
    branch_new(&zig, "main");
    write_file(&zig, "src/root.zig", "pub const x = 2;\n");
    commit_all(&zig, "mainline");
    let main_sha = rev(&zig, "HEAD");
    checkout(&zig, "develop");

    let cons = init(&root, "eb-staged");
    let hash = "1220deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let zon = |bare_sha: &str| {
        format!(
            r#"
.{{
    .name = .eb_staged,
    .version = "0.1.0",
    .dependencies = .{{
        .tagged = .{{ .url = "git+https://github.com/eessi/zig-common?ref=v0.9.0", .hash = "{hash}", }},
        .on_main = .{{ .url = "git+https://github.com/eessi/zig-common?ref=main#{main_sha}", .hash = "{hash}", }},
        .on_develop = .{{ .url = "git+https://github.com/eessi/zig-common?ref=develop#{dev_sha}", .hash = "{hash}", }},
        .bare = .{{ .url = "git+https://github.com/eessi/zig-common#{bare_sha}", .hash = "{hash}", }},
        .zig_common = .{{ .url = "git+https://github.com/eessi/zig-common?ref=v0.10.0", .hash = "{hash}", }},
    }},
}}
"#
        )
    };
    write_file(&cons, "build.zig.zon", &zon(&dev_sha));
    commit_all(&cons, "develop zon");
    branch_new(&cons, "main");
    write_file(&cons, "build.zig.zon", &zon(&main_sha));
    commit_all(&cons, "main zon");
    checkout(&cons, "develop");
    branch_new(&cons, "bump");
    ahead(&cons);

    let ws = r#"
[[node]]
id = "zig-v010"
kind = "gate"
gate = "tag"
repo = "zig-common"
path = "zig-common"
tag = "v0.10.0"

[[node]]
id = "eb-staged"
repo = "eb-staged"
path = "eb-staged"
base = "develop"
branch = "bump"
"#;
    let pending = plan(&root, ws, &[]);
    assert_eq!(pending.code, 0, "{}", pending.report);
    has(&pending.report, "spec: tag v0.9.0");
    has(&pending.report, "spec: branch main");
    has(&pending.report, "spec: branch develop");
    has(&pending.report, &format!("spec: commit {bare}"));
    has(&pending.report, &format!("develop pin: {dev_sha}"));
    has(&pending.report, &format!("main pin: {main_sha}"));
    has(&pending.script, "v0.10.0 is not present");
    not_has(&pending.script, "zig fetch");

    git(&zig, &["tag", "v0.10.0"]);
    let ready = plan(&root, ws, &[]);
    assert_eq!(ready.code, 0, "{}", ready.report);
    has(&ready.script, "zig fetch --save=zig_common");
    not_has(&ready.script, hash);
    not_has(&ready.script, "is not present");

    let root = tmp();
    let ridley = init(&root, "ridley");
    write_file(&ridley, "src/lib.rs", "pub fn core() {}\n");
    commit_all(&ridley, "core");
    let (old, new) = same_tree(&ridley);
    branch_new(&ridley, "main");
    write_file(&ridley, "src/lib.rs", "pub fn fix() {}\n");
    commit_all(&ridley, "fix");
    let fix = rev(&ridley, "HEAD");
    git(&ridley, &["update-ref", "refs/heads/develop", &new]);
    git(&ridley, &["update-ref", "refs/heads/main", &new]);
    checkout(&ridley, "develop");

    let desk = init(&root, "desktop");
    write_file(
        &desk,
        "Cargo.toml",
        &cargo_pkg(
            "desktop",
            &format!("[dependencies]\nridley-core = {{ git = \"https://github.com/ttech/ridley\", rev = \"{old}\" }}\n"),
        ),
    );
    write_file(&desk, "Cargo.lock", &format!("ridley-core {old}\n"));
    write_file(&desk, "ridley-core.rev", &format!("{old}\n"));
    commit_all(&desk, "pin");
    branch_new(&desk, "desk");
    ahead(&desk);

    let bad = init(&root, "badcomp");
    write_file(
        &bad,
        "Cargo.toml",
        &cargo_pkg(
            "badcomp",
            &format!("[dependencies]\nridley-core = {{ git = \"https://github.com/ttech/ridley\", rev = \"{old}\" }}\n"),
        ),
    );
    write_file(
        &bad,
        "ridley-core.rev",
        "ffffffffffffffffffffffffffffffffffffffff\n",
    );
    commit_all(&bad, "bad");
    branch_new(&bad, "bad");
    ahead(&bad);

    let missing_sha = "0123456789abcdef0123456789abcdef01234567";
    let miss = init(&root, "missing");
    write_file(
        &miss,
        "Cargo.toml",
        &cargo_pkg(
            "missing",
            &format!("[dependencies]\nridley-core = {{ git = \"https://github.com/ttech/ridley\", rev = \"{missing_sha}\" }}\n"),
        ),
    );
    commit_all(&miss, "miss");
    branch_new(&miss, "missing");
    ahead(&miss);

    let fixc = init(&root, "fixpin");
    write_file(
        &fixc,
        "Cargo.toml",
        &cargo_pkg(
            "fixpin",
            &format!("[dependencies]\nridley-core = {{ git = \"https://github.com/ttech/ridley\", rev = \"{fix}\" }}\n"),
        ),
    );
    commit_all(&fixc, "fix");
    branch_new(&fixc, "fixpin");
    ahead(&fixc);

    let wsrepo = init(&root, "ws");
    write_file(
        &wsrepo,
        "Cargo.toml",
        &format!(
            "[workspace]\nmembers = [\"crates/extra\"]\n\n[patch.crates-io]\nlinkcell = {{ git = \"https://github.com/ttech/ridley\", rev = \"{old}\" }}\n"
        ),
    );
    write_file(
        &wsrepo,
        "crates/extra/Cargo.toml",
        &format!(
            "[package]\nname = \"extra\"\nversion = \"0.1.0\"\n\n[patch.crates-io]\nother = {{ git = \"https://github.com/ttech/ridley\", rev = \"{old}\" }}\n"
        ),
    );
    commit_all(&wsrepo, "ws");

    let ws = format!(
        r#"
fix_sha = ["{fix}"]

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
companions = ["ridley-core.rev"]

[[node]]
id = "badcomp"
repo = "badcomp"
path = "badcomp"
base = "develop"
branch = "bad"
companions = ["ridley-core.rev"]

[[node]]
id = "missing"
repo = "missing"
path = "missing"
base = "develop"
branch = "missing"

[[node]]
id = "fixpin"
repo = "fixpin"
path = "fixpin"
base = "develop"
branch = "fixpin"

[[node]]
id = "ws"
repo = "ws"
path = "ws"
base = "develop"
branch = "develop"
"#
    );
    let out = plan(&root, &ws, &[]);
    assert_eq!(out.code, 0, "{}", out.report);
    has(&out.script, "cargo update -p ridley-core");
    has(&out.script, &new);
    let add = out
        .script
        .lines()
        .find(|l| {
            l.contains("Cargo.toml") && l.contains("Cargo.lock") && l.contains("ridley-core.rev")
        })
        .unwrap_or("");
    assert!(!add.is_empty(), "{}", out.script);
    has(&out.report, "ignored: crates/extra/Cargo.toml");
    has(&out.report, "rewrite: Cargo.toml");
    not_has(&out.script, "crates/extra/Cargo.toml");
    has(&out.report, "companions disagree");
    has(&out.report, "pin sha absent");
    has(&out.report, "fix sha");
    not_has(&out.script, "HEAD:bad");
    not_has(&out.script, "HEAD:missing");
    not_has(&out.script, "HEAD:fixpin");
}

#[test]
fn step_04_meson() {
    let root = tmp();
    let link = init(&root, "linkcell");
    write_file(&link, "src/lib.rs", "pub fn x() {}\n");
    commit_all(&link, "lib");
    let (old, new) = same_tree(&link);
    branch_new(&link, "main");
    let seams = init(&root, "seams");
    write_file(
        &seams,
        "subprojects/linkcell.wrap",
        &format!(
            "[wrap-git]\nurl = https://github.com/lode-org/linkcell.git\nrevision = {old}\ndepth = 1\n"
        ),
    );
    write_file(
        &seams,
        "subprojects/eigen.wrap",
        "[wrap-git]\nurl = https://github.com/eigen/eigen.git\nrevision = v3.4.0\n",
    );
    commit_all(&seams, "wraps");
    branch_new(&seams, "bump");
    ahead(&seams);
    let ws = r#"
[[node]]
id = "linkcell"
repo = "linkcell"
path = "linkcell"
base = "develop"
branch = "main"

[[node]]
id = "seams"
repo = "seams"
path = "seams"
base = "develop"
branch = "bump"
"#;
    let out = plan(&root, ws, &[]);
    assert_eq!(out.code, 0, "{}", out.report);
    has(&out.report, "depth = 1");
    has(&out.report, &new);
    has(&out.script, &format!("revision = {new}"));
    has(&out.script, "# depth = 1");
    let on_disk = fs::read_to_string(seams.join("subprojects/linkcell.wrap")).unwrap();
    has(&on_disk, "depth = 1");
    assert!(!out
        .report
        .lines()
        .any(|l| l.starts_with("edge:") && l.contains("eigen")));

    let root = tmp();
    let up = init(&root, "potentials-schema");
    write_file(&up, "README", "schema\n");
    commit_all(&up, "schema");
    let cons = init(&root, "cpmdc");
    let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    let wrap = format!(
        "[wrap-file]\nsource_url = https://github.com/lode-org/potentials-schema\nsource_hash = {hash}\n"
    );
    write_file(&cons, "subprojects/potentials.wrap", &wrap);
    commit_all(&cons, "wrap");
    branch_new(&cons, "bump");
    ahead(&cons);
    let before = fs::read(cons.join("subprojects/potentials.wrap")).unwrap();
    let stub = root.join("hash-stub");
    fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&stub).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&stub, p).unwrap();
    }
    let with = plan(
        &root,
        &format!(
            "hash_command = \"{}\"\n\n[[node]]\nid = \"potentials-schema\"\nrepo = \"potentials-schema\"\npath = \"potentials-schema\"\nbase = \"develop\"\nbranch = \"develop\"\n\n[[node]]\nid = \"cpmdc\"\nrepo = \"cpmdc\"\npath = \"cpmdc\"\nbase = \"develop\"\nbranch = \"bump\"\n",
            stub.display()
        ),
        &[],
    );
    assert_eq!(with.code, 0, "{}", with.report);
    has(&with.script, "hash-stub");
    not_has(&with.script, hash);

    let without = plan(
        &root,
        r#"
[[node]]
id = "potentials-schema"
repo = "potentials-schema"
path = "potentials-schema"
base = "develop"
branch = "develop"

[[node]]
id = "cpmdc"
repo = "cpmdc"
path = "cpmdc"
base = "develop"
branch = "bump"
"#,
        &[],
    );
    assert_eq!(without.code, 0, "{}", without.report);
    has(&without.report, "hash command is absent");
    assert_eq!(
        fs::read(cons.join("subprojects/potentials.wrap")).unwrap(),
        before
    );

    let root = tmp();
    let link = init(&root, "linkcell");
    branch_new(&link, "fix/exception-typeinfo-r3");
    write_file(&link, "src/lib.rs", "pub fn a() {}\n");
    commit_all(&link, "a");
    let old = rev(&link, "HEAD");
    write_file(&link, "src/lib.rs", "pub fn b() {}\n");
    commit_all(&link, "b");
    let seams = init(&root, "seams");
    write_file(
        &seams,
        "subprojects/linkcell.wrap",
        &format!(
            "[wrap-git]\nurl = https://github.com/lode-org/linkcell.git\nrevision = {old}\ndepth = 1\ndiff_files = linkcell-exception-typeinfo.patch\n"
        ),
    );
    write_file(
        &seams,
        "conda/recipe.sh",
        "patch -p1 < linkcell-exception-typeinfo.patch\n",
    );
    commit_all(&seams, "wrap");
    branch_new(&seams, "bump");
    ahead(&seams);
    let open = plan(
        &root,
        r#"
[[node]]
id = "linkcell"
repo = "linkcell"
path = "linkcell"
base = "develop"
branch = "fix/exception-typeinfo-r3"

[[node]]
id = "seams"
repo = "seams"
path = "seams"
base = "develop"
branch = "bump"
"#,
        &[],
    );
    assert_eq!(open.code, 0, "{}", open.report);
    has(&open.report, "subprojects/linkcell.wrap");
    has(&open.report, "conda/recipe.sh");
    has(&open.report, "still required");
    not_has(&open.script, "linkcell-exception-typeinfo.patch");

    fs::write(
        root.join("run.toml"),
        "[[done]]\nid = \"linkcell\"\nlanded = true\n",
    )
    .unwrap();
    let dropped = plan(
        &root,
        r#"
[[node]]
id = "linkcell"
repo = "linkcell"
path = "linkcell"
base = "develop"
branch = "fix/exception-typeinfo-r3"

[[node]]
id = "seams"
repo = "seams"
path = "seams"
base = "develop"
branch = "bump"
drop = true
"#,
        &["--run", root.join("run.toml").to_str().unwrap()],
    );
    assert_eq!(dropped.code, 0, "{}", dropped.report);
    has(&dropped.script, "diff_files");
    has(&dropped.script, "conda/recipe.sh");
    has(&dropped.script, "linkcell-exception-typeinfo.patch");
}

#[test]
fn step_05_registry_publish() {
    let root = tmp();
    let repo = init(&root, "rfc9162");
    write_file(&repo, "src/lib.rs", "pub fn x() {}\n");
    commit_all(&repo, "lib");
    git(&repo, &["tag", "v0.8.1"]);
    let cons = init(&root, "eb-stack");
    write_file(
        &cons,
        "Cargo.toml",
        &cargo_pkg("eb-stack", "[dependencies]\nrfc9162 = \"=0.8.1\"\n"),
    );
    commit_all(&cons, "pin");
    branch_new(&cons, "bump");
    ahead(&cons);
    let base = r#"
[[node]]
id = "publish-rfc9162"
kind = "gate"
gate = "publish"
repo = "rfc9162"
path = "rfc9162"
crate = "rfc9162"
version = "0.8.1"

[[node]]
id = "eb-stack"
repo = "eb-stack"
path = "eb-stack"
base = "develop"
branch = "bump"
"#;
    let empty = plan(&root, base, &[]);
    assert_eq!(empty.code, 0, "{}", empty.report);
    has(&empty.report, "tag is not a publish");
    not_has(&empty.script, "cargo update -p rfc9162");

    let recorded = plan(
        &root,
        &format!("[[publish]]\ncrate = \"rfc9162\"\nversion = \"0.8.1\"\n{base}"),
        &[],
    );
    assert_eq!(recorded.code, 0, "{}", recorded.report);
    has(&recorded.script, "cargo update -p rfc9162");

    let root = tmp();
    let core = init(&root, "readcon-core");
    write_file(&core, "src/lib.rs", "pub fn x() {}\n");
    commit_all(&core, "lib");
    let db = init(&root, "readcon-db");
    write_file(
        &db,
        "Cargo.toml",
        &cargo_pkg("readcon-db", "[dependencies]\nreadcon-core = \"=0.23.2\"\n"),
    );
    commit_all(&db, "pin");
    branch_new(&db, "bump");
    ahead(&db);
    let out = plan(
        &root,
        r#"
[[node]]
id = "publish-readcon-core"
kind = "gate"
gate = "publish"
repo = "readcon-core"
path = "readcon-core"
crate = "readcon-core"
version = "0.23.2"

[[node]]
id = "readcon-db"
repo = "readcon-db"
path = "readcon-db"
base = "develop"
branch = "bump"
"#,
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.report);
    has(
        &out.report,
        "edge: publish-readcon-core -> readcon-db registry",
    );

    let root = tmp();
    let rgmin = init(&root, "rgmin");
    write_file(&rgmin, "src/lib.rs", "pub fn x() {}\n");
    commit_all(&rgmin, "lib");
    let cons = init(&root, "rgflux");
    write_file(
        &cons,
        "Cargo.toml",
        &cargo_pkg("rgflux", "[dependencies]\nrgmin = \"=0.8.0\"\n"),
    );
    commit_all(&cons, "pin");
    branch_new(&cons, "bump");
    ahead(&cons);
    let ok = root.join("ok");
    fs::write(&ok, "#!/bin/sh\nexit 0\n").unwrap();
    let bad = root.join("bad");
    fs::write(&bad, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for pth in [&ok, &bad] {
            let mut p = fs::metadata(pth).unwrap().permissions();
            p.set_mode(0o755);
            fs::set_permissions(pth, p).unwrap();
        }
    }
    let gate = r#"
[[node]]
id = "publish-rgmin"
kind = "gate"
gate = "publish"
repo = "rgmin"
path = "rgmin"
crate = "rgmin"
version = "0.8.0"

[[node]]
id = "rgflux"
repo = "rgflux"
path = "rgflux"
base = "develop"
branch = "bump"
"#;
    let pass = plan(
        &root,
        &format!("publish_check = \"{}\"\n{gate}", ok.display()),
        &[],
    );
    assert_eq!(pass.code, 0, "{}", pass.report);
    has(&pass.script, "cargo update -p rgmin");

    git(&rgmin, &["tag", "v0.8.0"]);
    let fail = plan(
        &root,
        &format!("publish_check = \"{}\"\n{gate}", bad.display()),
        &[],
    );
    assert_eq!(fail.code, 0, "{}", fail.report);
    has(&fail.report, "tag is not a publish");
    not_has(&fail.script, "cargo update -p rgmin");

    let root = tmp();
    let saddle = init(&root, "rgsaddle");
    branch_new(&saddle, "topic");
    write_file(&saddle, "src/lib.rs", "pub fn topic() {}\n");
    commit_all(&saddle, "topic");
    let topic = rev(&saddle, "HEAD");
    git(&saddle, &["tag", "v0.6.0"]);
    let amsel = init(&root, "amsel");
    write_file(
        &amsel,
        "Cargo.toml",
        &cargo_pkg(
            "amsel",
            &format!("[dependencies]\nrgsaddle = {{ git = \"https://github.com/lode-org/rgsaddle\", rev = \"{topic}\" }}\n"),
        ),
    );
    commit_all(&amsel, "pin");
    branch_new(&amsel, "bump");
    ahead(&amsel);
    let out = plan(
        &root,
        r#"
[[node]]
id = "publish-rgsaddle"
kind = "gate"
gate = "publish"
repo = "rgsaddle"
path = "rgsaddle"
crate = "rgsaddle"
version = "0.6.0"

[[node]]
id = "rgsaddle-260"
repo = "rgsaddle"
path = "rgsaddle"
base = "develop"
branch = "topic"

[[node]]
id = "amsel"
repo = "amsel"
path = "amsel"
base = "develop"
branch = "bump"
"#,
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.report);
    has(&out.report, "edge: rgsaddle-260 -> amsel cargo");
    has(&out.report, "tag is not a publish");
    assert!(!out
        .report
        .lines()
        .any(|l| l.starts_with("edge:") && l.contains("rgsaddle-260") && l.contains("registry")));
}

