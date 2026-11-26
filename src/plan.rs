use crate::emit::{script_of, sh_quote, Block, EdgeRep, NodeRep, PinRep, Report};
use crate::gitutil;
use crate::model::{RunFile, Workspace};
use crate::scan::{self, Pin};
use clap::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "kitline", about = "Plan a kit landing from a workspace file")]
struct Cli {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long)]
    report: PathBuf,
    #[arg(long)]
    script: PathBuf,
    #[arg(long)]
    public_report: Option<PathBuf>,
    #[arg(long)]
    run: Option<PathBuf>,
    #[arg(long)]
    public: bool,
    #[arg(long)]
    strict: bool,
    #[arg(long)]
    tickets: bool,
    /// Read-only git ls-remote. Off for a local plan.
    #[arg(long)]
    online: bool,
}

struct Live {
    id: String,
    kind: String,
    repo: String,
    dir: PathBuf,
    branch: String,
    base_ref: String,
    base_sha: String,
    target_sha: String,
    head_sha: String,
    tree: String,
    lease: String,
    lease_ok: bool,
    claimed_tree: String,
    ahead: u64,
    behind: u64,
    have_counts: bool,
    mt_clean: bool,
    mt_files: Vec<String>,
    pr: Option<u64>,
    pr_state: String,
    base_pr: Option<u64>,
    base_pr_state: String,
    private: bool,
    land: bool,
    open_pr: bool,
    fix: bool,
    drop: bool,
    merge: String,
    sync: String,
    needs: Vec<String>,
    supersedes: String,
    superseded_by: String,
    companions: Vec<String>,
    generated: Vec<String>,
    gate: String,
    tag: String,
    crate_name: String,
    version: String,
    url: String,
    refused: Vec<String>,
    notes: Vec<String>,
    keep_head: Vec<String>,
    done: bool,
    suppress: bool,
}

struct Rewrite {
    consumer: String,
    kind: String,
    dep: String,
    old: String,
    new_sha: String,
    file: String,
    extra: Vec<String>,
    depth: String,
    url: String,
    recipes: Vec<String>,
}

pub fn run() -> i32 {
    execute(Cli::parse())
}

fn execute(cli: Cli) -> i32 {
    let public_path = cli.public_report.clone().unwrap_or_else(|| {
        let mut p = cli.report.clone();
        p.set_extension("public.org");
        p
    });
    match build(&cli) {
        Err(err) => {
            let text = format!("#+title: kitline report\n\n* Error\n\n{err}\n");
            let _ = write_text(&cli.report, &text);
            let _ = fs::remove_file(&cli.script);
            1
        }
        Ok(rep) => {
            let local = crate::emit::render(&rep, false);
            if write_text(&cli.report, &local).is_err() {
                return 1;
            }
            if cli.public {
                let public = crate::emit::render(&rep, true);
                if write_text(&public_path, &public).is_err() {
                    return 1;
                }
            }
            if rep.write_script {
                if write_text(&cli.script, &rep.script).is_err() {
                    return 1;
                }
            } else {
                let _ = fs::remove_file(&cli.script);
            }
            rep.exit
        }
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

fn build(cli: &Cli) -> Result<Report, String> {
    let ws_text = fs::read_to_string(&cli.workspace).map_err(|e| format!("bad file: {e}"))?;
    let ws: Workspace = toml::from_str(&ws_text).map_err(|e| format!("parse error: {e}"))?;
    let ws_dir = cli
        .workspace
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let run = if let Some(path) = &cli.run {
        let text = fs::read_to_string(path).map_err(|e| format!("bad run file: {e}"))?;
        toml::from_str::<RunFile>(&text).map_err(|e| format!("parse error: {e}"))?
    } else {
        RunFile::default()
    };
    let check = ws.publish_check.as_ref().map(|cmd| run_cmd(cmd));

    let mut lives = Vec::new();
    for node in &ws.node {
        lives.push(inspect(node, &ws, &ws_dir)?);
    }
    mark_done(&mut lives, &run);
    mark_publish(&mut lives, &ws, check);

    let mut reasons = Vec::new();
    let mut exit2 = false;
    pass_refusals(&mut lives);
    let mut edges: Vec<(String, String, String)> = Vec::new();
    let mut rewrites: Vec<Rewrite> = Vec::new();
    let mut pins_out: Vec<PinRep> = Vec::new();
    let mut notes = Vec::new();
    if !cli.online {
        notes.push("on remote: skipped".into());
    }

    mark_supersede(&mut lives);
    add_ancestry(&lives, &mut edges);
    add_needs(&lives, &mut edges);
    if cli.tickets {
        if let Some(path) = &ws.tickets {
            scan_tickets(&ws_dir.join(path), &lives, &mut edges);
        }
    }
    scan_depends(&lives, &mut edges);

    let mut seen_dirs = BTreeSet::new();
    for i in 0..lives.len() {
        if lives[i].gate == "tag" || lives[i].gate == "publish" || lives[i].head_sha.is_empty() {
            continue;
        }
        if !seen_dirs.insert(lives[i].dir.clone()) {
            continue;
        }
        notes.extend(branch_pin_notes(&lives[i].dir));
        let found = scan::scan_rev(&lives[i].dir, &lives[i].head_sha, &ws.metadata_rev);
        let consumer = lives[i].id.clone();
        let private = lives[i].private;
        for pin in found {
            pins_out.push(PinRep {
                consumer: consumer.clone(),
                file: pin.file.clone(),
                line: pin.line,
                spec: pin.spec.clone(),
                dep: pin.dep.clone(),
                note: String::new(),
                private,
            });
            match_pin(
                &ws,
                &run,
                check,
                &mut lives,
                i,
                &pin,
                &mut edges,
                &mut rewrites,
                &mut notes,
                &mut exit2,
                &mut reasons,
                cli.online,
            );
        }
    }

    apply_claims(
        &mut lives,
        &ws,
        &mut edges,
        &mut notes,
        &mut exit2,
        &mut reasons,
    );
    apply_contracts(&mut lives, &ws, &mut notes, &mut exit2, &mut reasons);
    apply_records(
        &ws,
        &lives,
        &mut edges,
        &mut notes,
        &mut exit2,
        &mut reasons,
    );
    note_publish_tags(&lives, &ws, check, &mut notes);
    sibling_notes(&lives, &mut notes);
    stacked_heads(&mut lives);
    restack_notes(&mut lives);

    let ids: Vec<String> = lives.iter().map(|n| n.id.clone()).collect();
    let graph: Vec<(String, String)> = edges
        .iter()
        .map(|(a, b, _)| (a.clone(), b.clone()))
        .collect();
    let cyc = cycle_nodes(&ids, &graph);
    let mut exit = 0;
    if !cyc.is_empty() {
        exit = 3;
        for id in &cyc {
            let kind = lives
                .iter()
                .find(|n| n.id == *id)
                .map(|n| n.kind.as_str())
                .unwrap_or("");
            notes.push(format!("cycle: {id} kind {kind}"));
            reasons.push(format!("cycle {id}"));
        }
    } else if exit2 {
        exit = 2;
    }

    let dropped = edges.iter().any(|(a, b, _)| {
        lives
            .iter()
            .any(|n| n.private && (n.id == *a || n.id == *b))
    });
    if cli.public && dropped {
        notes.push("omitted edges: private".into());
        if exit == 0 {
            exit = 2;
        }
        reasons.push("public report dropped an edge".into());
    }
    if exit == 0 && cli.strict && lives.iter().any(|n| !n.refused.is_empty()) {
        exit = 2;
        reasons.push("strict".into());
    }

    let ready = ready_ids(&lives, &edges);
    let mut blocks = Vec::new();
    for id in &ready {
        let Some(n) = lives.iter().find(|n| n.id == *id) else {
            continue;
        };
        let lines = block_lines(n, &rewrites, &ws);
        if lines.is_empty() {
            continue;
        }
        blocks.push(Block {
            id: id.clone(),
            lines,
        });
    }
    let batch = if ready.is_empty() {
        Vec::new()
    } else {
        vec![format!("batch 1 unordered: {}", ready.join(", "))]
    };
    let script = script_of(&blocks);
    let write_script = exit == 0 && !blocks.is_empty();

    let nodes = lives
        .iter()
        .map(|n| NodeRep {
            id: n.id.clone(),
            kind: n.kind.clone(),
            repo: n.repo.clone(),
            branch: n.branch.clone(),
            base_ref: n.base_ref.clone(),
            base_sha: n.base_sha.clone(),
            head_sha: n.head_sha.clone(),
            lease: n.lease.clone(),
            lease_match: if n.lease_ok { "yes" } else { "no" }.into(),
            tree: n.tree.clone(),
            claimed_tree: n.claimed_tree.clone(),
            ahead: if n.have_counts {
                n.ahead.to_string()
            } else {
                String::new()
            },
            behind: if n.have_counts {
                n.behind.to_string()
            } else {
                String::new()
            },
            merge_tree: if n.head_sha.is_empty() || n.kind == "gate" {
                String::new()
            } else if n.mt_clean {
                "clean".into()
            } else {
                "conflict".into()
            },
            merge_files: n.mt_files.join(" "),
            pr: n.pr.map(|p| p.to_string()).unwrap_or_default(),
            pr_state: n.pr_state.clone(),
            base_pr: n.base_pr.map(|p| p.to_string()).unwrap_or_default(),
            base_pr_state: n.base_pr_state.clone(),
            superseded_by: n.superseded_by.clone(),
            supersedes: n.supersedes.clone(),
            landed: if n.done { "yes" } else { "no" }.into(),
            refused: n.refused.clone(),
            notes: n.notes.clone(),
            merge: n.merge.clone(),
            private: n.private,
            keep_head: n.keep_head.clone(),
        })
        .collect();
    let edge_reps = edges
        .into_iter()
        .map(|(from, to, kind)| EdgeRep { from, to, kind })
        .collect();
    Ok(Report {
        exit,
        reasons,
        nodes,
        edges: edge_reps,
        pins: pins_out,
        notes,
        batches: batch,
        blocks,
        ci: ws.ci.unwrap_or_default(),
        manuals: ws.manual,
        script,
        write_script,
    })
}

fn run_cmd(cmd: &str) -> i32 {
    Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .ok()
        .and_then(|s| s.code())
        .unwrap_or(1)
}

fn abs_path(base: &Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    let joined = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    if joined.is_absolute() {
        joined
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(joined)
    }
}

fn inspect(node: &crate::model::Node, ws: &Workspace, ws_dir: &Path) -> Result<Live, String> {
    let repo = node.repo.clone().unwrap_or_else(|| node.id.clone());
    let rel = node
        .path
        .clone()
        .or_else(|| ws.repos.get(&repo).map(|r| r.path.clone()))
        .unwrap_or_else(|| repo.clone());
    let dir = abs_path(ws_dir, &rel);
    if !dir.exists() {
        return Err(format!("missing clone {}", dir.display()));
    }
    let kind = if let Some(k) = &node.kind {
        k.clone()
    } else if node.gate.is_some() {
        "gate".into()
    } else if node.open_pr.unwrap_or(false) || node.pr.is_some() {
        "pr".into()
    } else {
        "kit".into()
    };
    let gate = node.gate.clone().unwrap_or_default();
    let branch = node.branch.clone().unwrap_or_default();
    let base_ref = node.base.clone().unwrap_or_default();
    if kind != "gate" && branch.is_empty() {
        return Err(format!("node {} has no branch", node.id));
    }
    if !branch.is_empty() && gitutil::rev(&dir, &branch).is_none() {
        return Err(format!("node {} missing ref {branch}", node.id));
    }
    if node.source.as_deref() == Some("bundle") && node.bundle.is_none() {
        return Err(format!(
            "node {} names a bundle and has no bundle path",
            node.id
        ));
    }
    if let Some(bundle) = &node.bundle {
        let path = abs_path(ws_dir, bundle);
        if !path.exists() {
            return Err(format!("missing bundle {}", path.display()));
        }
        gitutil::verify_bundle(&dir, &path)?;
    }
    let head_sha = if branch.is_empty() {
        String::new()
    } else {
        gitutil::rev(&dir, &branch).unwrap_or_default()
    };
    let target_sha = if base_ref.is_empty() {
        String::new()
    } else {
        gitutil::rev(&dir, &base_ref).unwrap_or_default()
    };
    let base_sha = if let Some(c) = &node.base_commit {
        gitutil::expand(&dir, c)
    } else {
        target_sha.clone()
    };
    let tree = if head_sha.is_empty() {
        String::new()
    } else {
        gitutil::tree_of(&dir, &head_sha).unwrap_or_default()
    };
    let bundle_lease = node.bundle.as_ref().and_then(|b| {
        gitutil::bundle_prereqs(&abs_path(ws_dir, b))
            .into_iter()
            .next()
    });
    let matches = node.lease_matches.as_deref().unwrap_or("head");
    let expected = if matches == "base" {
        base_sha.clone()
    } else {
        head_sha.clone()
    };
    let recorded = node
        .lease
        .clone()
        .or(bundle_lease)
        .map(|s| gitutil::expand(&dir, &s));
    let (lease, lease_ok) = if kind == "gate" {
        (String::new(), true)
    } else if let Some(got) = recorded {
        (got.clone(), got == expected)
    } else {
        (expected.clone(), true)
    };
    let claimed_tree = node.tree.clone().unwrap_or_default();
    let (ahead, behind, have_counts) = if !target_sha.is_empty() && !head_sha.is_empty() {
        match gitutil::ahead_behind(&dir, &target_sha, &head_sha) {
            Some((a, b)) => (a, b, true),
            None => (0, 0, false),
        }
    } else {
        (0, 0, false)
    };
    let (mt_clean, mt_files) =
        if kind == "gate" || head_sha.is_empty() || target_sha.is_empty() || head_sha == target_sha
        {
            (true, Vec::new())
        } else {
            gitutil::merge_tree(&dir, &target_sha, &head_sha)
        };
    let mut companions = node.companions.clone();
    for c in &ws.companion {
        if c.node == node.id {
            companions.extend(c.paths.clone());
        }
    }
    Ok(Live {
        id: node.id.clone(),
        kind,
        repo,
        dir,
        branch,
        base_ref,
        base_sha,
        target_sha,
        head_sha,
        tree,
        lease,
        lease_ok,
        claimed_tree,
        ahead,
        behind,
        have_counts,
        mt_clean,
        mt_files,
        pr: node.pr,
        pr_state: node.pr_state.clone().unwrap_or_default(),
        base_pr: node.base_pr,
        base_pr_state: node.base_pr_state.clone().unwrap_or_default(),
        private: node.private.unwrap_or(false),
        land: node.land.unwrap_or(true),
        open_pr: node.open_pr.unwrap_or(false),
        fix: node.fix.unwrap_or(false),
        drop: node.drop.unwrap_or(false),
        merge: node.merge.clone().unwrap_or_default(),
        sync: node.sync.clone().unwrap_or_default(),
        needs: node.needs.clone(),
        supersedes: node.supersedes.clone().unwrap_or_default(),
        superseded_by: String::new(),
        companions,
        generated: node.generated.clone(),
        gate,
        tag: node.tag.clone().unwrap_or_default(),
        crate_name: node.crate_name.clone().unwrap_or_default(),
        version: node.version.clone().unwrap_or_default(),
        url: node.url.clone().unwrap_or_default(),
        refused: Vec::new(),
        notes: Vec::new(),
        keep_head: Vec::new(),
        done: false,
        suppress: false,
    })
}

fn mark_done(lives: &mut [Live], run: &RunFile) {
    for n in lives.iter_mut() {
        let from_run = run
            .done
            .iter()
            .any(|d| d.id == n.id && d.landed.unwrap_or(true));
        if n.gate == "tag" {
            let tagged = !n.tag.is_empty() && gitutil::tag_exists(&n.dir, &n.tag);
            n.done = from_run || tagged;
            continue;
        }
        if n.gate == "publish" {
            n.done = from_run;
            continue;
        }
        if from_run {
            n.done = true;
            continue;
        }
        if !n.head_sha.is_empty()
            && !n.target_sha.is_empty()
            && gitutil::is_ancestor(&n.dir, &n.head_sha, &n.target_sha)
        {
            n.done = true;
            continue;
        }
        if same_tree_landed(n) {
            n.done = true;
        }
    }
}

fn mark_publish(lives: &mut [Live], ws: &Workspace, check: Option<i32>) {
    for n in lives.iter_mut() {
        if n.gate != "publish" {
            continue;
        }
        if check == Some(0) || publish_record(ws, &n.crate_name, &n.version) {
            n.done = true;
        }
    }
}

fn same_tree_landed(n: &Live) -> bool {
    if n.tree.is_empty() || n.target_sha.is_empty() || n.base_sha.is_empty() {
        return false;
    }
    let base_tree = gitutil::tree_of(&n.dir, &n.base_sha).unwrap_or_default();
    if n.tree == base_tree {
        return false;
    }
    tree_on(&n.dir, &n.target_sha, &n.tree)
}

fn tree_on(dir: &Path, target: &str, tree: &str) -> bool {
    let out = gitutil::git(dir, &["rev-list", "--format=%T", target]);
    out.stdout.lines().any(|l| l.trim() == tree)
}

fn pass_refusals(lives: &mut [Live]) {
    let open_parents: Vec<(String, String, String)> = lives
        .iter()
        .filter(|n| n.pr_state != "closed" && n.superseded_by.is_empty() && !n.branch.is_empty())
        .map(|n| (n.repo.clone(), n.branch.clone(), n.id.clone()))
        .collect();
    for n in lives.iter_mut() {
        if n.kind == "gate" {
            continue;
        }
        let open_parent = open_parents
            .iter()
            .any(|(repo, branch, id)| repo == &n.repo && branch == &n.base_ref && id != &n.id);
        if !n.lease_ok {
            n.refused.push("lease mismatch".into());
        }
        if !n.claimed_tree.is_empty() && !n.lease.is_empty() {
            let actual = gitutil::tree_of(&n.dir, &n.lease).unwrap_or_else(|| n.tree.clone());
            if actual != n.claimed_tree {
                n.refused.push("tree mismatch".into());
                n.notes.push(format!(
                    "tree mismatch: {actual} claimed {}",
                    n.claimed_tree
                ));
            }
        }
        if !n.base_ref.is_empty() && n.target_sha.is_empty() {
            n.refused.push(format!("unresolved base {}", n.base_ref));
        }
        if !n.base_sha.is_empty()
            && !n.target_sha.is_empty()
            && !gitutil::is_ancestor(&n.dir, &n.base_sha, &n.target_sha)
            && !open_parent
        {
            n.refused.push(format!(
                "ancestor failure: fatal: base commit {} is not an ancestor of {}",
                n.base_sha, n.target_sha
            ));
        }
        if n.base_pr_state == "closed" && !open_parent {
            let num = n
                .base_pr
                .map(|p| p.to_string())
                .unwrap_or_else(|| "closed".into());
            n.refused.push(format!("base pull request {num} is closed"));
        }
        if n.pr_state == "closed" {
            n.refused.push("pull request is closed".into());
        }
        if !n.mt_clean {
            n.refused.push("merge-tree conflict".into());
            for g in &n.generated {
                if n.mt_files.iter().any(|f| f == g) {
                    n.notes.push(format!("regenerate: {g}"));
                }
            }
        }
        if n.fix {
            n.refused.push("fix node".into());
        }
    }
}

fn mark_supersede(lives: &mut [Live]) {
    let pairs: Vec<(String, String)> = lives
        .iter()
        .filter(|n| !n.supersedes.is_empty())
        .map(|n| (n.id.clone(), n.supersedes.clone()))
        .collect();
    for (new_id, old_id) in pairs {
        if let Some(old) = lives.iter_mut().find(|n| n.id == old_id) {
            old.superseded_by = new_id;
        }
    }
}

fn add_ancestry(lives: &[Live], edges: &mut Vec<(String, String, String)>) {
    for child in lives {
        if child.kind == "gate" || child.head_sha.is_empty() || !child.superseded_by.is_empty() {
            continue;
        }
        let mut parents = Vec::new();
        for parent in lives {
            if parent.id == child.id || parent.repo != child.repo || parent.kind == "gate" {
                continue;
            }
            if !parent.superseded_by.is_empty() || parent.head_sha.is_empty() {
                continue;
            }
            if gitutil::strict_ancestor(&child.dir, &parent.head_sha, &child.head_sha) {
                parents.push(parent.id.clone());
            }
        }
        let mut immediate = Vec::new();
        for p in &parents {
            let p_head = lives
                .iter()
                .find(|n| n.id == *p)
                .map(|n| n.head_sha.clone());
            let Some(p_head) = p_head else { continue };
            let between = parents.iter().any(|m| {
                if m == p {
                    return false;
                }
                let m_head = lives
                    .iter()
                    .find(|n| n.id == *m)
                    .map(|n| n.head_sha.as_str())
                    .unwrap_or("");
                gitutil::strict_ancestor(&child.dir, &p_head, m_head)
                    && gitutil::strict_ancestor(&child.dir, m_head, &child.head_sha)
            });
            if !between {
                immediate.push(p.clone());
            }
        }
        for p in immediate {
            add_edge(edges, &p, &child.id, "ancestry");
        }
    }
}

fn add_needs(lives: &[Live], edges: &mut Vec<(String, String, String)>) {
    let ids: BTreeSet<&str> = lives.iter().map(|n| n.id.as_str()).collect();
    for n in lives {
        for need in &n.needs {
            if ids.contains(need.as_str()) {
                add_edge(edges, need, &n.id, "needs");
            }
        }
    }
}

fn add_edge(edges: &mut Vec<(String, String, String)>, from: &str, to: &str, kind: &str) {
    if from.is_empty() || to.is_empty() || from == to {
        return;
    }
    if edges
        .iter()
        .any(|(a, b, k)| a == from && b == to && k == kind)
    {
        return;
    }
    edges.push((from.to_string(), to.to_string(), kind.to_string()));
}

fn scan_tickets(path: &Path, lives: &[Live], edges: &mut Vec<(String, String, String)>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(":DEPENDS:") else {
            continue;
        };
        for n in lives {
            if rest.contains(&n.id) {
                for m in lives {
                    if m.id != n.id && rest.contains(&m.id) {
                        add_edge(edges, &n.id, &m.id, "needs");
                    }
                }
            }
        }
    }
}

fn scan_depends(lives: &[Live], edges: &mut Vec<(String, String, String)>) {
    for n in lives {
        if n.head_sha.is_empty() || n.base_sha.is_empty() {
            continue;
        }
        for sha in gitutil::commits_on(&n.dir, &n.base_sha, &n.head_sha) {
            let msg = gitutil::commit_message(&n.dir, &sha);
            for line in msg.lines() {
                let low = line.to_ascii_lowercase();
                if !(low.starts_with("depends-on:") || low.starts_with("depends-on ")) {
                    continue;
                }
                for other in lives {
                    if other.id != n.id && line.contains(&other.id) {
                        add_edge(edges, &other.id, &n.id, "needs");
                    }
                }
            }
        }
    }
}

enum PinSha {
    Unique(String),
    Missing,
    Ambiguous,
}

fn resolve_pin_sha(dir: &Path, sha: &str) -> PinSha {
    let short = sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit());
    if short {
        let out = gitutil::git(dir, &["rev-parse", &format!("--disambiguate={sha}")]);
        let mut commits = Vec::new();
        for line in out.stdout.lines() {
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            if gitutil::rev(dir, &format!("{name}^{{commit}}")).as_deref() == Some(name) {
                commits.push(name.to_string());
            }
        }
        return match commits.len() {
            0 => PinSha::Missing,
            1 => PinSha::Unique(commits.pop().unwrap_or_default()),
            _ => PinSha::Ambiguous,
        };
    }
    match gitutil::rev(dir, sha) {
        Some(full) => PinSha::Unique(full),
        None => PinSha::Missing,
    }
}

#[allow(clippy::too_many_arguments)]
fn match_branch(
    run: &RunFile,
    lives: &[Live],
    consumer_idx: usize,
    pin: &Pin,
    branch: &str,
    edges: &mut Vec<(String, String, String)>,
    rewrites: &mut Vec<Rewrite>,
    notes: &mut Vec<String>,
    exit2: &mut bool,
    reasons: &mut Vec<String>,
) {
    if branch.is_empty() {
        return;
    }
    let slug = scan::slug(&pin.url);
    let ups: Vec<usize> = (0..lives.len())
        .filter(|i| slug_hit(&lives[*i], &slug, &pin.url))
        .collect();
    if ups.is_empty() {
        notes.push(format!("pin unmatched: {}", pin.dep));
        return;
    }
    let cid = lives[consumer_idx].id.clone();
    let floating = ups
        .iter()
        .any(|&i| lives[i].kind != "gate" && lives[i].base_ref == branch);
    if floating {
        let mut linked = false;
        for i in &ups {
            let n = &lives[*i];
            if n.kind == "gate" || n.base_ref != branch || n.branch == branch || n.branch.is_empty()
            {
                continue;
            }
            add_edge(edges, &n.id, &cid, "branch");
            linked = true;
        }
        if linked {
            let kind = if pin.family == "zig" { "zig" } else { "cargo" };
            rewrites.push(Rewrite {
                consumer: cid,
                kind: kind.into(),
                dep: pin.dep.clone(),
                old: String::new(),
                new_sha: String::new(),
                file: pin.file.clone(),
                extra: Vec::new(),
                depth: String::new(),
                url: pin.url.clone(),
                recipes: Vec::new(),
            });
        }
        return;
    }
    let topics: Vec<usize> = ups
        .iter()
        .copied()
        .filter(|&i| lives[i].kind != "gate" && lives[i].branch == branch)
        .collect();
    if topics.len() > 1 {
        *exit2 = true;
        let ids: Vec<&str> = topics.iter().map(|i| lives[*i].id.as_str()).collect();
        let line = format!("ambiguous: {}", ids.join(" "));
        reasons.push(line.clone());
        notes.push(line);
        return;
    }
    let Some(up) = topics.first().copied() else {
        return;
    };
    let landed = landed_sha(&lives[up], run);
    let gid = lives[up].id.clone();
    let kind = if pin.family == "zig" { "zig" } else { "cargo" };
    add_edge(edges, &gid, &cid, kind);
    if landed.is_empty() {
        return;
    }
    rewrites.push(Rewrite {
        consumer: cid,
        kind: "branch-sha".into(),
        dep: pin.dep.clone(),
        old: branch.to_string(),
        new_sha: landed,
        file: pin.file.clone(),
        extra: Vec::new(),
        depth: String::new(),
        url: String::new(),
        recipes: Vec::new(),
    });
}

fn sh_commit(lines: &mut Vec<String>, paths: &[String], message: &str) {
    if paths.is_empty() {
        return;
    }
    let listed = paths
        .iter()
        .map(|p| sh_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!("git add -- {listed}"));
    lines.push(format!(
        "if ! git diff --cached --quiet; then git commit -m {}; fi",
        sh_quote(message)
    ));
}

fn sh_replace(lines: &mut Vec<String>, expr: &str, file: &str) {
    let q = sh_quote(file);
    lines.push(format!("sed {} {q} > {q}.new", sh_quote(expr)));
    lines.push(format!("mv {q}.new {q}"));
}

#[allow(clippy::too_many_arguments)]
fn match_pin(
    ws: &Workspace,
    run: &RunFile,
    check: Option<i32>,
    lives: &mut [Live],
    consumer_idx: usize,
    pin: &Pin,
    edges: &mut Vec<(String, String, String)>,
    rewrites: &mut Vec<Rewrite>,
    notes: &mut Vec<String>,
    exit2: &mut bool,
    reasons: &mut Vec<String>,
    online: bool,
) {
    if pin.section.starts_with("patch") && !pin.root {
        notes.push(format!("ignored: {}", pin.file));
        return;
    }
    if pin.path_dep {
        let kept = kept_crate(ws, &pin.dep);
        if pin.section.starts_with("patch") && !kept {
            notes.push(format!("unmarked patch: {}", pin.dep));
        }
        return;
    }
    if pin.family == "registry" {
        let ver = scan::version_key(&pin.version);
        if let Some(g) = find_publish(lives, &pin.dep, &ver) {
            let gid = lives[g].id.clone();
            let cid = lives[consumer_idx].id.clone();
            add_edge(edges, &gid, &cid, "registry");
            if lives[g].done || publish_record(ws, &pin.dep, &ver) || check == Some(0) {
                rewrites.push(Rewrite {
                    consumer: cid,
                    kind: "cargo-ver".into(),
                    dep: pin.dep.clone(),
                    old: String::new(),
                    new_sha: String::new(),
                    file: pin.file.clone(),
                    extra: Vec::new(),
                    depth: String::new(),
                    url: String::new(),
                    recipes: Vec::new(),
                });
            }
        }
        return;
    }
    if pin.family == "meson" && pin.section == "wrap-file" {
        let cid = lives[consumer_idx].id.clone();
        if let Some(g) = find_slug(lives, &pin.url) {
            let gid = lives[g].id.clone();
            add_edge(edges, &gid, &cid, "meson");
            if ws.hash_command.is_none() {
                lives[consumer_idx]
                    .refused
                    .push("hash command is absent".into());
            } else {
                rewrites.push(Rewrite {
                    consumer: cid,
                    kind: "hash".into(),
                    dep: pin.dep.clone(),
                    old: String::new(),
                    new_sha: String::new(),
                    file: pin.file.clone(),
                    extra: Vec::new(),
                    depth: String::new(),
                    url: String::new(),
                    recipes: Vec::new(),
                });
            }
        } else {
            notes.push(format!("pin unmatched: {}", pin.dep));
        }
        return;
    }
    if let Some(branch) = pin.spec.strip_prefix("branch ") {
        match_branch(
            run,
            lives,
            consumer_idx,
            pin,
            branch.trim(),
            edges,
            rewrites,
            notes,
            exit2,
            reasons,
        );
        return;
    }
    if pin.spec.starts_with("tag ")
        || (pin.family == "meson" && !pin.version.is_empty() && pin.sha.is_empty())
    {
        let tag = pin
            .spec
            .strip_prefix("tag ")
            .unwrap_or(pin.version.as_str())
            .to_string();
        let slug = scan::slug(&pin.url);
        if let Some(g) = find_tag(lives, &slug, &tag) {
            let gid = lives[g].id.clone();
            let cid = lives[consumer_idx].id.clone();
            add_edge(edges, &gid, &cid, "tag");
            if lives[g].done {
                rewrites.push(Rewrite {
                    consumer: cid,
                    kind: "zig".into(),
                    dep: pin.dep.clone(),
                    old: String::new(),
                    new_sha: String::new(),
                    file: pin.file.clone(),
                    extra: Vec::new(),
                    depth: String::new(),
                    url: pin.url.clone(),
                    recipes: Vec::new(),
                });
            }
        }
        return;
    }
    if pin.sha.is_empty() {
        if pin.family == "meson" {
            notes.push(format!("pin unmatched: {}", pin.dep));
        }
        return;
    }
    if is_fix(&ws.fix_sha, &pin.sha) {
        lives[consumer_idx]
            .refused
            .push(format!("fix sha {}", pin.sha));
        return;
    }
    let slug = scan::slug(&pin.url);
    let ups: Vec<usize> = (0..lives.len())
        .filter(|i| slug_hit(&lives[*i], &slug, &pin.url))
        .collect();
    if ups.is_empty() {
        notes.push(format!("pin unmatched: {}", pin.dep));
        return;
    }
    let dir = lives[ups[0]].dir.clone();
    let full = match resolve_pin_sha(&dir, &pin.sha) {
        PinSha::Unique(full) => full,
        PinSha::Missing => {
            lives[consumer_idx]
                .refused
                .push(format!("pin sha absent {}", pin.sha));
            return;
        }
        PinSha::Ambiguous => {
            *exit2 = true;
            reasons.push(format!("ambiguous: {}", pin.sha));
            notes.push(format!("ambiguous: {}", pin.sha));
            return;
        }
    };
    if online {
        let out = gitutil::git(&dir, &["ls-remote", "origin", &full]);
        let on = if out.stdout.contains(&full) {
            "yes"
        } else {
            "no"
        };
        notes.push(format!("on remote: {on} {full}"));
    }
    let tree = gitutil::tree_of(&dir, &full).unwrap_or_default();
    let mut contained = Vec::new();
    for i in &ups {
        let n = &lives[*i];
        if n.head_sha.is_empty() || n.kind == "gate" {
            continue;
        }
        let on_head = gitutil::is_ancestor(&dir, &full, &n.head_sha);
        let on_base = !n.base_sha.is_empty() && gitutil::is_ancestor(&dir, &full, &n.base_sha);
        if on_head && !on_base {
            contained.push(*i);
        }
    }
    if contained.len() > 1 {
        if let Some(follow) = ws
            .follows
            .iter()
            .find(|f| full.starts_with(&f.sha) || f.sha == full)
        {
            if let Some(i) = lives.iter().position(|n| n.id == follow.node) {
                contained = vec![i];
            }
        }
    }
    if contained.len() > 1 {
        *exit2 = true;
        let ids: Vec<&str> = contained.iter().map(|i| lives[*i].id.as_str()).collect();
        reasons.push(format!("ambiguous: {}", ids.join(" ")));
        notes.push(format!("ambiguous: {}", ids.join(" ")));
        return;
    }
    if contained.is_empty() {
        let on_base = ups.iter().any(|i| {
            let n = &lives[*i];
            (!n.base_sha.is_empty() && gitutil::is_ancestor(&dir, &full, &n.base_sha))
                || (!n.target_sha.is_empty() && gitutil::is_ancestor(&dir, &full, &n.target_sha))
        });
        let on_named = ["develop", "main"].iter().any(|name| {
            gitutil::rev(&dir, name)
                .map(|tip| gitutil::is_ancestor(&dir, &full, &tip))
                .unwrap_or(false)
        });
        if on_base || on_named {
            let dev = ups
                .iter()
                .copied()
                .find(|&i| lives[i].base_ref == "develop" || lives[i].branch == "develop");
            if let Some(dev) = dev {
                notes.push(format!("pin sha: {full}"));
                notes.push(format!("upstream develop: {}", lives[dev].head_sha));
            }
            if let Some(tag) = gitutil::latest_tag(&dir) {
                notes.push(format!("latest tag: {tag}"));
            }
            return;
        }
        let mut hits = Vec::new();
        for i in &ups {
            if lives[*i].kind == "gate" {
                continue;
            }
            if node_has_tree(&lives[*i], &tree, run) {
                hits.push(*i);
            }
        }
        if hits.len() > 1 {
            *exit2 = true;
            let ids: Vec<&str> = hits.iter().map(|i| lives[*i].id.as_str()).collect();
            reasons.push(format!("ambiguous: {}", ids.join(" ")));
            notes.push(format!("ambiguous: {}", ids.join(" ")));
            return;
        }
        if hits.is_empty() {
            lives[consumer_idx]
                .refused
                .push(format!("pin sha absent {}", pin.sha));
            return;
        }
        contained = hits;
    }
    let up = contained[0];
    let landed = landed_sha(&lives[up], run);
    let gid = lives[up].id.clone();
    let cid = lives[consumer_idx].id.clone();
    let kind = if pin.family == "meson" {
        "meson"
    } else if pin.family == "zig" {
        "zig"
    } else {
        "cargo"
    };
    if pin.section.starts_with("patch") && publish_gate_idx(lives, &pin.dep).is_some() {
        let satisfied = publish_satisfied(lives, ws, check, &pin.dep);
        if !satisfied {
            notes.push(format!("patch: {}", pin.dep));
            add_edge(edges, &gid, &cid, kind);
            return;
        }
        let drop = lives[consumer_idx].drop || ws.drop_crate.iter().any(|c| c == &pin.dep);
        if !drop {
            lives[consumer_idx]
                .refused
                .push(format!("git patch {} remains and drop is not set", pin.dep));
            return;
        }
        if let Some(g) = publish_gate_idx(lives, &pin.dep) {
            let gate_id = lives[g].id.clone();
            add_edge(edges, &gate_id, &cid, "registry");
        }
        rewrites.push(Rewrite {
            consumer: cid,
            kind: "drop-table".into(),
            dep: pin.dep.clone(),
            old: String::new(),
            new_sha: String::new(),
            file: pin.file.clone(),
            extra: Vec::new(),
            depth: String::new(),
            url: String::new(),
            recipes: Vec::new(),
        });
        return;
    }
    if landed == full {
        add_edge(edges, &gid, &cid, kind);
        return;
    }
    if !check_companions(lives, consumer_idx, &full) {
        return;
    }
    let mut extra = companion_paths(lives, consumer_idx, &full);
    if let Some(text) = gitutil::show_text(
        &lives[consumer_idx].dir,
        &lives[consumer_idx].head_sha,
        "Cargo.lock",
    ) {
        if text.contains(&full) {
            extra.push("Cargo.lock".into());
        }
    }
    let mut recipes = Vec::new();
    if !pin.diff_files.is_empty() {
        let head = lives[consumer_idx].head_sha.clone();
        let cdir = lives[consumer_idx].dir.clone();
        for name in &pin.diff_files {
            notes.push(format!("patch files: {} {name}", pin.file));
            for file in gitutil::list_files(&cdir, &head) {
                if file == pin.file {
                    continue;
                }
                if let Some(text) = gitutil::show_text(&cdir, &head, &file) {
                    if text.contains(name) {
                        recipes.push(file);
                    }
                }
            }
        }
        for r in &recipes {
            notes.push(format!("patch files: {r}"));
        }
        if !lives[up].done {
            notes.push("still required".into());
        }
    }
    if !pin.depth.is_empty() {
        notes.push(format!("depth: {}", pin.depth));
        notes.push(format!(
            "planned:\nrevision = {landed}\ndepth = {}",
            pin.depth
        ));
    }
    let rewrite_kind = if pin.section == "metadata" {
        "meta"
    } else if pin.family == "meson" {
        "meson"
    } else if pin.family == "zig" {
        "zig"
    } else {
        "cargo"
    };
    if pin.section.starts_with("patch") && pin.root {
        notes.push(format!("rewrite: {}", pin.file));
    }
    rewrites.push(Rewrite {
        consumer: cid.clone(),
        kind: rewrite_kind.into(),
        dep: pin.dep.clone(),
        old: if pin.sha.is_empty() {
            full.clone()
        } else {
            pin.sha.clone()
        },
        new_sha: landed.clone(),
        file: pin.file.clone(),
        extra,
        depth: pin.depth.clone(),
        url: pin.url.clone(),
        recipes: recipes.clone(),
    });
    if pin.family == "meson"
        && lives[up].done
        && (lives[consumer_idx].drop || ws.drop_crate.iter().any(|c| c == &pin.dep))
    {
        rewrites.push(Rewrite {
            consumer: cid.clone(),
            kind: "drop-diff".into(),
            dep: pin.dep.clone(),
            old: String::new(),
            new_sha: String::new(),
            file: pin.file.clone(),
            extra: pin.diff_files.clone(),
            depth: String::new(),
            url: String::new(),
            recipes,
        });
    }
    add_edge(edges, &gid, &cid, kind);
}

fn kept_crate(ws: &Workspace, name: &str) -> bool {
    if ws.keep.iter().any(|k| k == name) {
        return true;
    }
    ws.crate_policy
        .iter()
        .any(|p| p.crate_name == name && (p.keep.unwrap_or(false) || p.upstream == Some(false)))
}

fn publish_record(ws: &Workspace, crate_name: &str, version: &str) -> bool {
    let v = scan::version_key(version);
    ws.publish
        .iter()
        .any(|p| p.crate_name == crate_name && scan::version_key(&p.version) == v)
}

fn find_publish(lives: &[Live], crate_name: &str, version: &str) -> Option<usize> {
    let v = scan::version_key(version);
    lives.iter().position(|n| {
        n.gate == "publish" && n.crate_name == crate_name && scan::version_key(&n.version) == v
    })
}

fn publish_gate_idx(lives: &[Live], crate_name: &str) -> Option<usize> {
    lives
        .iter()
        .position(|n| n.gate == "publish" && n.crate_name == crate_name)
}

fn publish_satisfied(lives: &[Live], ws: &Workspace, check: Option<i32>, crate_name: &str) -> bool {
    if check == Some(0) {
        return true;
    }
    if let Some(i) = publish_gate_idx(lives, crate_name) {
        if lives[i].done {
            return true;
        }
        if publish_record(ws, crate_name, &lives[i].version) {
            return true;
        }
    }
    false
}

fn find_tag(lives: &[Live], slug: &str, tag: &str) -> Option<usize> {
    lives.iter().position(|n| {
        n.gate == "tag" && n.tag == tag && (n.repo == slug || slug.is_empty() || n.repo.is_empty())
    })
}

fn find_slug(lives: &[Live], url: &str) -> Option<usize> {
    let slug = scan::slug(url);
    lives.iter().position(|n| slug_hit(n, &slug, url))
}

fn slug_hit(n: &Live, slug: &str, url: &str) -> bool {
    if slug.is_empty() {
        return false;
    }
    if n.repo == slug {
        return true;
    }
    if !n.url.is_empty() && (scan::slug(&n.url) == slug || url.contains(&n.url)) {
        return true;
    }
    false
}

fn is_fix(list: &[String], sha: &str) -> bool {
    list.iter().any(|f| {
        let f = f.trim();
        sha == f || (f.len() >= 7 && sha.starts_with(f))
    })
}

fn node_has_tree(n: &Live, tree: &str, run: &RunFile) -> bool {
    if tree.is_empty() {
        return false;
    }
    if run
        .done
        .iter()
        .any(|d| d.id == n.id && d.tree.as_deref() == Some(tree))
    {
        return true;
    }
    if n.tree == tree {
        return true;
    }
    for sha in gitutil::commits_on(&n.dir, &n.base_sha, &n.head_sha) {
        if gitutil::tree_of(&n.dir, &sha).as_deref() == Some(tree) {
            return true;
        }
    }
    false
}

fn landed_sha(n: &Live, run: &RunFile) -> String {
    if let Some(d) = run.done.iter().find(|d| d.id == n.id) {
        if let Some(s) = d.sha.clone().or(d.tip.clone()) {
            return s;
        }
    }
    n.head_sha.clone()
}

fn check_companions(lives: &mut [Live], idx: usize, sha: &str) -> bool {
    let paths = lives[idx].companions.clone();
    let dir = lives[idx].dir.clone();
    let head = lives[idx].head_sha.clone();
    for path in paths {
        let text = gitutil::show_text(&dir, &head, &path).unwrap_or_default();
        if !text.contains(sha) {
            lives[idx]
                .refused
                .push(format!("companions disagree {path}"));
            return false;
        }
    }
    true
}

fn companion_paths(lives: &[Live], idx: usize, sha: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in &lives[idx].companions {
        if let Some(text) = gitutil::show_text(&lives[idx].dir, &lives[idx].head_sha, path) {
            if text.contains(sha) {
                out.push(path.clone());
            }
        }
    }
    out
}

fn apply_claims(
    lives: &mut [Live],
    ws: &Workspace,
    edges: &mut Vec<(String, String, String)>,
    notes: &mut Vec<String>,
    exit2: &mut bool,
    reasons: &mut Vec<String>,
) {
    let mut groups: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for claim in &ws.claim {
        let Some(n) = lives.iter().find(|n| n.id == claim.node) else {
            continue;
        };
        let target = gitutil::show_text(&n.dir, &n.base_ref, &claim.path).unwrap_or_default();
        let head = gitutil::show_text(&n.dir, &n.head_sha, &claim.path).unwrap_or_default();
        let cur = scan::read_symbol(&target, &claim.symbol).unwrap_or(0);
        let Some(val) = scan::read_symbol(&head, &claim.symbol) else {
            continue;
        };
        if val <= cur {
            notes.push(format!(
                "claim: {} value {val} already on the target",
                claim.node
            ));
            continue;
        }
        groups
            .entry(claim.symbol.clone())
            .or_default()
            .push((claim.node.clone(), val));
    }
    for (_symbol, mut rows) in groups {
        rows.sort_by_key(|r| r.1);
        let mut seen: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for (id, val) in &rows {
            seen.entry(*val).or_default().push(id.clone());
        }
        let mut conflict = false;
        for (val, ids) in &seen {
            if ids.len() > 1 {
                conflict = true;
                *exit2 = true;
                notes.push(format!("claim conflict: {} value {val}", ids.join(" ")));
                reasons.push(format!("claim conflict {}", ids.join(" ")));
            }
        }
        if conflict {
            continue;
        }
        for (i, (id, val)) in rows.iter().enumerate() {
            if i + 1 < rows.len() {
                notes.push(format!("claim batch: {id} value {val}"));
            } else if rows.len() > 1 {
                notes.push(format!("claim batch: {id} value {val} re-merge"));
            } else {
                notes.push(format!("claim batch: {id} value {val}"));
            }
            if i > 0 {
                add_edge(edges, &rows[i - 1].0, id, "claim");
            }
        }
    }
}

fn apply_contracts(
    lives: &mut [Live],
    ws: &Workspace,
    notes: &mut Vec<String>,
    exit2: &mut bool,
    reasons: &mut Vec<String>,
) {
    for c in &ws.contract {
        if c.compare == "integer" {
            notes.push(format!("ungated: {}", c.consumer));
            continue;
        }
        if c.compare == "exact" {
            let prod = lives.iter().find(|n| n.id == c.producer);
            let cons = lives.iter().find(|n| n.id == c.consumer);
            if let (Some(p), Some(n), Some(pp), Some(ps), Some(cp), Some(cs)) = (
                prod,
                cons,
                c.producer_path.as_ref(),
                c.producer_symbol.as_ref(),
                c.consumer_path.as_ref(),
                c.consumer_symbol.as_ref(),
            ) {
                let pt = gitutil::show_text(&p.dir, &p.head_sha, pp).unwrap_or_default();
                let ct = gitutil::show_text(&n.dir, &n.head_sha, cp).unwrap_or_default();
                let a = scan::read_symbol(&pt, ps);
                let b = scan::read_symbol(&ct, cs);
                notes.push(format!("schema: {pp}"));
                notes.push(format!("schema: {cp}"));
                if let Some(label) = &c.label {
                    notes.push(label.clone());
                }
                if a != b {
                    *exit2 = true;
                    reasons.push(format!("schema mismatch {pp} {cp}"));
                }
            }
            continue;
        }
        if c.compare == "callers" {
            if let Some(n) = lives.iter_mut().find(|n| n.id == c.consumer) {
                let names = gitutil::diff_names(&n.dir, &n.base_sha, &n.head_sha);
                let touches = c.paths.iter().any(|p| names.iter().any(|n| n == p));
                if !touches {
                    n.suppress = true;
                    n.notes.push("caller paths untouched".into());
                }
            }
        }
    }
}

fn apply_records(
    ws: &Workspace,
    lives: &[Live],
    edges: &mut Vec<(String, String, String)>,
    notes: &mut Vec<String>,
    exit2: &mut bool,
    reasons: &mut Vec<String>,
) {
    let Some(rec) = &ws.records else {
        return;
    };
    let Some(host) = lives
        .iter()
        .find(|n| n.repo == rec.repo || n.id == rec.repo)
    else {
        return;
    };
    let rev = rec.rev.clone().unwrap_or_else(|| host.head_sha.clone());
    let mut kids: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for file in gitutil::list_files(&host.dir, &rev) {
        if !file.starts_with(&rec.directory) || !file.ends_with(".json") {
            continue;
        }
        let Some(text) = gitutil::show_text(&host.dir, &rev, &file) else {
            continue;
        };
        let Some(parent) = scan::json_field(&text, &rec.field) else {
            continue;
        };
        let stem = Path::new(&file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        kids.entry(parent).or_default().push((stem, file));
    }
    for (parent, children) in kids {
        if children.len() > 1 {
            *exit2 = true;
            let paths: Vec<&str> = children.iter().map(|(_, p)| p.as_str()).collect();
            notes.push(format!("supersedes fork: {}", paths.join(" ")));
            reasons.push(format!("supersedes fork {}", paths.join(" ")));
            continue;
        }
        for (child, _) in &children {
            add_edge(edges, &parent, child, "record");
            notes.push(format!("record edge: {parent} -> {child}"));
        }
    }
}

fn note_publish_tags(lives: &[Live], ws: &Workspace, check: Option<i32>, notes: &mut Vec<String>) {
    for n in lives {
        if n.gate != "publish" {
            continue;
        }
        let ver = scan::version_key(&n.version);
        let tag = format!("v{ver}");
        let present = gitutil::tag_exists(&n.dir, &tag) || gitutil::tag_exists(&n.dir, &ver);
        if present {
            notes.push(format!("tag present: {tag}"));
        }
        if publish_record(ws, &n.crate_name, &n.version) {
            notes.push(format!("publish record: {} {}", n.crate_name, n.version));
        } else {
            notes.push(format!("publish record: empty {}", n.crate_name));
        }
        if let Some(code) = check {
            notes.push(format!("publish check: {code}"));
        }
        if present && !publish_record(ws, &n.crate_name, &n.version) && check != Some(0) {
            notes.push("tag is not a publish".into());
        }
    }
}

fn sibling_notes(lives: &[Live], notes: &mut Vec<String>) {
    for i in 0..lives.len() {
        for j in (i + 1)..lives.len() {
            let a = &lives[i];
            let b = &lives[j];
            if a.repo != b.repo || a.kind == "gate" || b.kind == "gate" {
                continue;
            }
            if a.head_sha.is_empty() || b.head_sha.is_empty() {
                continue;
            }
            if gitutil::strict_ancestor(&a.dir, &a.head_sha, &b.head_sha)
                || gitutil::strict_ancestor(&a.dir, &b.head_sha, &a.head_sha)
            {
                continue;
            }
            let (clean, files) = gitutil::merge_tree(&a.dir, &a.head_sha, &b.head_sha);
            if !clean {
                notes.push(format!(
                    "sibling: {} {} file: {}",
                    a.id,
                    b.id,
                    files.join(" ")
                ));
            }
        }
    }
}

fn restack_notes(lives: &mut [Live]) {
    let parents: Vec<(String, String, String, String)> = lives
        .iter()
        .filter(|n| !n.branch.is_empty())
        .map(|n| {
            (
                n.repo.clone(),
                n.branch.clone(),
                n.head_sha.clone(),
                n.id.clone(),
            )
        })
        .collect();
    for n in lives.iter_mut() {
        if n.base_ref.is_empty() || n.lease.is_empty() {
            continue;
        }
        for (repo, branch, head, id) in &parents {
            if repo == &n.repo
                && branch == &n.base_ref
                && id != &n.id
                && !head.is_empty()
                && n.lease != *head
                && !gitutil::strict_ancestor(&n.dir, head, &n.head_sha)
            {
                n.notes
                    .push(format!("restack: {} {} {head}", n.id, n.lease));
            }
        }
    }
}

fn stacked_heads(lives: &mut [Live]) {
    let kids: Vec<(String, String, String)> = lives
        .iter()
        .filter(|n| !n.base_ref.is_empty() && !n.branch.is_empty())
        .map(|n| (n.repo.clone(), n.base_ref.clone(), n.id.clone()))
        .collect();
    for n in lives.iter_mut() {
        if n.branch.is_empty() {
            continue;
        }
        for (repo, base, id) in &kids {
            if repo == &n.repo && base == &n.branch && id != &n.id {
                n.keep_head.push(format!(
                    "{} because stacked pull request {id} still bases on it",
                    n.branch
                ));
            }
        }
    }
}

fn branch_pin_notes(dir: &Path) -> Vec<String> {
    if !gitutil::ref_exists(dir, "main") || !gitutil::ref_exists(dir, "develop") {
        return Vec::new();
    }
    let mut notes = Vec::new();
    for rev in ["develop", "main"] {
        let sha = gitutil::rev(dir, rev).unwrap_or_default();
        for file in gitutil::list_files(dir, rev) {
            if !(file.ends_with("build.zig.zon") || file.ends_with("Cargo.toml")) {
                continue;
            }
            let Some(text) = gitutil::show_text(dir, rev, &file) else {
                continue;
            };
            for hex in hex40(&text) {
                notes.push(format!("{rev} pin: {hex}"));
            }
        }
        let _ = sha;
    }
    notes
}

fn hex40(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_hexdigit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 40 {
                out.push(chars[start..i].iter().collect());
            }
        } else {
            i += 1;
        }
    }
    out
}

fn cycle_nodes(ids: &[String], edges: &[(String, String)]) -> Vec<String> {
    let mut indeg: BTreeMap<&str, usize> = ids.iter().map(|id| (id.as_str(), 0)).collect();
    let mut adj: BTreeMap<&str, Vec<&str>> =
        ids.iter().map(|id| (id.as_str(), Vec::new())).collect();
    for (from, to) in edges {
        if indeg.contains_key(from.as_str()) && indeg.contains_key(to.as_str()) {
            adj.get_mut(from.as_str()).unwrap().push(to.as_str());
            *indeg.get_mut(to.as_str()).unwrap() += 1;
        }
    }
    let mut q: Vec<&str> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    q.sort_unstable();
    let mut seen = 0usize;
    while let Some(n) = q.pop() {
        seen += 1;
        let mut nexts = adj[n].clone();
        nexts.sort_unstable();
        for m in nexts {
            let d = indeg.get_mut(m).unwrap();
            *d -= 1;
            if *d == 0 {
                q.push(m);
            }
        }
    }
    if seen == ids.len() {
        return Vec::new();
    }
    let mut left: Vec<String> = indeg
        .into_iter()
        .filter(|(_, d)| *d > 0)
        .map(|(k, _)| k.to_string())
        .collect();
    left.sort();
    left
}

fn ready_ids(lives: &[Live], edges: &[(String, String, String)]) -> Vec<String> {
    let mut ids: Vec<String> = lives
        .iter()
        .filter(|n| ready_one(n, lives, edges))
        .map(|n| n.id.clone())
        .collect();
    ids.sort_by(|a, b| {
        let na = lives.iter().find(|n| n.id == *a).unwrap();
        let nb = lives.iter().find(|n| n.id == *b).unwrap();
        na.repo
            .cmp(&nb.repo)
            .then(na.branch.cmp(&nb.branch))
            .then(na.id.cmp(&nb.id))
    });
    ids
}

fn ready_one(n: &Live, lives: &[Live], edges: &[(String, String, String)]) -> bool {
    if n.suppress || n.fix || !n.superseded_by.is_empty() || !n.refused.is_empty() {
        return false;
    }
    let preds_done = edges
        .iter()
        .filter(|(_, to, _)| to == &n.id)
        .all(|(from, _, _)| {
            lives
                .iter()
                .find(|p| p.id == *from)
                .map(|p| p.done)
                .unwrap_or(true)
        });
    if !preds_done {
        return false;
    }
    if n.gate == "publish" {
        return false;
    }
    if n.gate == "tag" {
        return !n.done;
    }
    if !n.land || n.done {
        return false;
    }
    true
}

fn block_lines(n: &Live, rewrites: &[Rewrite], ws: &Workspace) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "cd {} || exit 1",
        sh_quote(&n.dir.to_string_lossy())
    ));
    if n.gate == "tag" {
        lines.push(format!(
            "printf '%s\\n' {}",
            sh_quote(&format!(
                "kitline: tag gate {} {} is not present",
                n.id, n.tag
            ))
        ));
        return lines;
    }
    if n.sync == "no-ff" && !n.base_ref.is_empty() {
        lines.push(format!("git merge --no-ff {}", sh_quote(&n.base_ref)));
    }
    for rw in rewrites.iter().filter(|r| r.consumer == n.id) {
        match rw.kind.as_str() {
            "cargo" => {
                lines.push(format!("cargo update -p {}", sh_quote(&rw.dep)));
                let mut paths = vec![rw.file.clone()];
                paths.extend(rw.extra.clone());
                if !rw.old.is_empty() && !rw.new_sha.is_empty() {
                    let expr = format!("s/{}/{}/g", rw.old, rw.new_sha);
                    for p in &paths {
                        sh_replace(&mut lines, &expr, p);
                    }
                }
                sh_commit(&mut lines, &paths, &format!("bump {}", rw.dep));
            }
            "cargo-ver" => {
                lines.push(format!("cargo update -p {}", sh_quote(&rw.dep)));
            }
            "zig" => {
                let url = if rw.url.is_empty() {
                    String::new()
                } else {
                    format!(" {}", sh_quote(&rw.url))
                };
                lines.push(format!("zig fetch --save={}{url}", sh_quote(&rw.dep)));
                sh_commit(
                    &mut lines,
                    std::slice::from_ref(&rw.file),
                    &format!("bump {}", rw.dep),
                );
            }
            "meson" => {
                if !rw.depth.is_empty() {
                    lines.push(format!("# depth = {}", sh_quote(&rw.depth)));
                }
                let expr = format!("s/^revision = .*/revision = {}/", rw.new_sha);
                sh_replace(&mut lines, &expr, &rw.file);
                sh_commit(
                    &mut lines,
                    std::slice::from_ref(&rw.file),
                    &format!("bump {}", rw.dep),
                );
            }
            "hash" => {
                if let Some(cmd) = &ws.hash_command {
                    lines.push(format!("{} {}", sh_quote(cmd), sh_quote(&rw.file)));
                    sh_commit(
                        &mut lines,
                        std::slice::from_ref(&rw.file),
                        &format!("bump {}", rw.dep),
                    );
                }
            }
            "drop-diff" => {
                sh_replace(&mut lines, "/^diff_files/d", &rw.file);
                let mut paths = vec![rw.file.clone()];
                for recipe in &rw.recipes {
                    for name in &rw.extra {
                        let base = name.rsplit('/').next().unwrap_or(name);
                        sh_replace(&mut lines, &format!("/{base}/d"), recipe);
                    }
                    paths.push(recipe.clone());
                }
                sh_commit(&mut lines, &paths, &format!("bump {}", rw.dep));
            }
            "drop-table" => {
                lines.push(
                    "awk 'BEGIN { skip = 0 } /^\\[patch\\.crates-io\\]/ { skip = 1; next } /^\\[/ { skip = 0 } skip == 0 { print }' Cargo.toml > Cargo.toml.new"
                        .into(),
                );
                lines.push("mv Cargo.toml.new Cargo.toml".into());
                lines.push("git add -- Cargo.toml".into());
                lines.push("git commit -m 'drop patch'".into());
            }
            "branch-sha" => {
                let spec = format!("{}:{}", n.head_sha, rw.file);
                let shown = gitutil::git(&n.dir, &["show", &spec]);
                let current = if shown.status == 0 {
                    shown.stdout
                } else {
                    String::new()
                };
                let from = format!("branch = \"{}\"", rw.old);
                let to = format!("rev = \"{}\"", rw.new_sha);
                let rewritten = current.replace(&from, &to);
                lines.push(format!(
                    "printf '%s' {} > {}",
                    sh_quote(&rewritten),
                    sh_quote(&rw.file)
                ));
                sh_commit(
                    &mut lines,
                    std::slice::from_ref(&rw.file),
                    &format!("bump {}", rw.dep),
                );
            }
            "meta" => {
                let expr = format!("s/{}/{}/g", rw.old, rw.new_sha);
                sh_replace(&mut lines, &expr, &rw.file);
                sh_commit(
                    &mut lines,
                    std::slice::from_ref(&rw.file),
                    &format!("bump {}", rw.dep),
                );
            }
            _ => {}
        }
    }
    if n.open_pr && n.pr.is_none() {
        let head = sh_quote(&n.branch);
        let base = sh_quote(&n.base_ref);
        lines.push(format!(
            "if ! gh pr list --head {head} --base {base} --json number | grep -q '\"number\"'; then"
        ));
        lines.push(format!(
            "gh pr create --head {head} --base {base} --title {head}"
        ));
        lines.push("fi".into());
    }
    if n.land && n.kind != "gate" && !n.branch.is_empty() {
        lines.push(format!("git push origin HEAD:{}", sh_quote(&n.branch)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Workspace;

    fn live_at(dir: &Path, sha: &str) -> Live {
        Live {
            id: "anneal".into(),
            kind: "kit".into(),
            repo: "anneal".into(),
            dir: dir.to_path_buf(),
            branch: "bump".into(),
            base_ref: "develop".into(),
            base_sha: sha.into(),
            target_sha: sha.into(),
            head_sha: sha.into(),
            tree: String::new(),
            lease: sha.into(),
            lease_ok: true,
            claimed_tree: String::new(),
            ahead: 1,
            behind: 0,
            have_counts: true,
            mt_clean: true,
            mt_files: Vec::new(),
            pr: None,
            pr_state: String::new(),
            base_pr: None,
            base_pr_state: String::new(),
            private: false,
            land: false,
            open_pr: false,
            fix: false,
            drop: false,
            merge: String::new(),
            sync: String::new(),
            needs: Vec::new(),
            supersedes: String::new(),
            superseded_by: String::new(),
            companions: Vec::new(),
            generated: Vec::new(),
            gate: String::new(),
            tag: String::new(),
            crate_name: String::new(),
            version: String::new(),
            url: String::new(),
            refused: Vec::new(),
            notes: Vec::new(),
            keep_head: Vec::new(),
            done: false,
            suppress: false,
        }
    }

    #[test]
    fn branch_rewrite_keeps_hostile_ref_text() {
        let dir = std::env::temp_dir().join(format!(
            "kitline-branch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?} {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "develop"]);
        git(&["config", "user.email", "rgoswami@ieee.org"]);
        git(&["config", "user.name", "Rohit Goswami"]);
        git(&["config", "commit.gpgsign", "false"]);
        let branch = "/&.|*[]\\\n";
        let body = format!("branch = \"{branch}\"\n");
        fs::write(dir.join("Cargo.toml"), &body).unwrap();
        git(&["add", "Cargo.toml"]);
        git(&["commit", "-qm", "pin"]);
        git(&["checkout", "-q", "-b", "bump"]);
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let new_sha = "0123456789abcdef0123456789abcdef01234567";
        let rw = Rewrite {
            consumer: "anneal".into(),
            kind: "branch-sha".into(),
            dep: "topic".into(),
            old: branch.to_string(),
            new_sha: new_sha.into(),
            file: "Cargo.toml".into(),
            extra: Vec::new(),
            depth: String::new(),
            url: String::new(),
            recipes: Vec::new(),
        };
        let ws: Workspace = toml::from_str("").unwrap();
        let lines = block_lines(&live_at(&dir, &sha), &[rw], &ws);
        let mut text = String::from("#!/bin/sh\nset -eu\n");
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
        assert!(!text.contains("sed "), "{text}");
        let script = dir.join("run.sh");
        fs::write(&script, &text).unwrap();
        let ran = Command::new("sh").arg(&script).status().unwrap();
        assert!(ran.success(), "{text}");
        let got = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert_eq!(got, format!("rev = \"{new_sha}\"\n"));
        let _ = fs::remove_dir_all(&dir);
    }
}
