#[derive(Clone, Debug)]
pub struct NodeRep {
    pub id: String,
    pub kind: String,
    pub repo: String,
    pub branch: String,
    pub base_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    pub lease: String,
    pub lease_match: String,
    pub tree: String,
    pub claimed_tree: String,
    pub ahead: String,
    pub behind: String,
    pub merge_tree: String,
    pub merge_files: String,
    pub pr: String,
    pub pr_state: String,
    pub base_pr: String,
    pub base_pr_state: String,
    pub superseded_by: String,
    pub supersedes: String,
    pub landed: String,
    pub refused: Vec<String>,
    pub notes: Vec<String>,
    pub merge: String,
    pub private: bool,
    pub keep_head: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EdgeRep {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct PinRep {
    pub consumer: String,
    pub file: String,
    pub line: usize,
    pub spec: String,
    pub dep: String,
    pub note: String,
    pub private: bool,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub exit: i32,
    pub reasons: Vec<String>,
    pub nodes: Vec<NodeRep>,
    pub edges: Vec<EdgeRep>,
    pub pins: Vec<PinRep>,
    pub notes: Vec<String>,
    pub batches: Vec<String>,
    pub blocks: Vec<Block>,
    pub ci: String,
    pub manuals: Vec<String>,
    pub script: String,
    pub write_script: bool,
}

pub fn render(rep: &Report, public: bool) -> String {
    let mut s = String::new();
    s.push_str("#+title: kitline report\n#+options: toc:nil num:nil ^:{}\n\n");
    s.push_str(&format!("* Summary\n\nexit: {}\n", rep.exit));
    if !rep.ci.is_empty() {
        s.push_str(&format!("ci: {}\n", rep.ci));
    }
    for r in &rep.reasons {
        s.push_str(&format!("reason: {r}\n"));
    }
    s.push('\n');
    s.push_str("* Nodes\n\n");
    for n in &rep.nodes {
        if public && n.private {
            continue;
        }
        s.push_str(&format!("** {}\n\n", n.id));
        s.push_str(&format!("kind: {}\n", n.kind));
        s.push_str(&format!("repo: {}\n", n.repo));
        s.push_str(&format!("branch: {}\n", n.branch));
        s.push_str(&format!("base ref: {}\n", n.base_ref));
        s.push_str(&format!("base sha: {}\n", n.base_sha));
        s.push_str(&format!("head sha: {}\n", n.head_sha));
        s.push_str(&format!("lease: {}\n", n.lease));
        s.push_str(&format!("lease match: {}\n", n.lease_match));
        s.push_str(&format!("tree: {}\n", n.tree));
        if !n.claimed_tree.is_empty() {
            s.push_str(&format!("claimed tree: {}\n", n.claimed_tree));
        }
        if !n.ahead.is_empty() {
            s.push_str(&format!("ahead: {}\n", n.ahead));
            s.push_str(&format!("behind: {}\n", n.behind));
        }
        if !n.merge_tree.is_empty() {
            s.push_str(&format!("merge-tree: {}\n", n.merge_tree));
        }
        if !n.merge_files.is_empty() {
            s.push_str(&format!("merge-tree files: {}\n", n.merge_files));
        }
        if !n.pr.is_empty() {
            s.push_str(&format!("pr: {}\n", n.pr));
        }
        if !n.pr_state.is_empty() {
            s.push_str(&format!("pr state: {}\n", n.pr_state));
        }
        if !n.base_pr.is_empty() {
            s.push_str(&format!("base pr: {}\n", n.base_pr));
        }
        if !n.base_pr_state.is_empty() {
            s.push_str(&format!("base pr state: {}\n", n.base_pr_state));
        }
        if !n.superseded_by.is_empty() {
            s.push_str(&format!("superseded-by: {}\n", n.superseded_by));
        }
        if !n.supersedes.is_empty() {
            s.push_str(&format!("supersedes: {}\n", n.supersedes));
        }
        s.push_str(&format!("landed: {}\n", n.landed));
        if !n.merge.is_empty() {
            s.push_str(&format!("merge: {}\n", n.merge));
        }
        for k in &n.keep_head {
            s.push_str(&format!("keep head: {k}\n"));
        }
        for r in &n.refused {
            s.push_str(&format!("refused: {r}\n"));
        }
        for note in &n.notes {
            s.push_str(&format!("{note}\n"));
        }
        s.push('\n');
    }
    s.push_str("* Edges\n\n");
    for e in &rep.edges {
        if public {
            let from_priv = rep.nodes.iter().any(|n| n.id == e.from && n.private);
            let to_priv = rep.nodes.iter().any(|n| n.id == e.to && n.private);
            if from_priv || to_priv {
                continue;
            }
        }
        s.push_str(&format!("edge: {} -> {} {}\n", e.from, e.to, e.kind));
    }
    s.push('\n');
    s.push_str("* Pins\n\n");
    for p in &rep.pins {
        if public && p.private {
            continue;
        }
        s.push_str(&format!(
            "pin: {} {}:{} spec: {} dep: {}\n",
            p.consumer, p.file, p.line, p.spec, p.dep
        ));
        if !p.note.is_empty() {
            s.push_str(&format!("{}\n", p.note));
        }
    }
    s.push('\n');
    if !rep.batches.is_empty() {
        s.push_str("* Batches\n\n");
        for b in &rep.batches {
            s.push_str(&format!("{b}\n"));
        }
        s.push('\n');
    }
    if !rep.notes.is_empty() {
        s.push_str("* Notes\n\n");
        for n in &rep.notes {
            s.push_str(&format!("{n}\n"));
        }
        s.push('\n');
    }
    if !rep.manuals.is_empty() {
        s.push_str("* Manual\n\n");
        for m in &rep.manuals {
            s.push_str(&format!("manual: {m}\n"));
        }
        s.push('\n');
    }
    if !rep.blocks.is_empty() {
        s.push_str("* Commands\n\n");
        for b in &rep.blocks {
            let priv_node = rep.nodes.iter().any(|n| n.id == b.id && n.private);
            if public && priv_node {
                continue;
            }
            s.push_str(&format!("command block: {}\n", b.id));
            for line in &b.lines {
                s.push_str(line);
                s.push('\n');
            }
            s.push('\n');
        }
    }
    s
}

pub fn script_of(blocks: &[Block]) -> String {
    let mut s = String::from("#!/bin/sh\nset -eu\n");
    for b in blocks {
        s.push_str(&format!("\n# node: {}\n", b.id));
        for line in &b.lines {
            s.push_str(line);
            s.push('\n');
        }
    }
    s.push('\n');
    s
}

pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
