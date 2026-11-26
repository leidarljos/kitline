use crate::gitutil;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Pin {
    pub file: String,
    pub line: usize,
    pub family: String,
    pub dep: String,
    pub spec: String,
    pub sha: String,
    pub version: String,
    pub url: String,
    pub depth: String,
    pub diff_files: Vec<String>,
    pub section: String,
    pub root: bool,
    pub raw_hash: String,
    pub path_dep: bool,
}

pub fn scan_rev(dir: &Path, rev: &str, metadata_rev: &[String]) -> Vec<Pin> {
    let mut out = Vec::new();
    for file in gitutil::list_files(dir, rev) {
        if file == ".cargo/config.toml" || file.ends_with("/.cargo/config.toml") {
            continue;
        }
        let Some(text) = gitutil::show_text(dir, rev, &file) else {
            continue;
        };
        if file == "build.zig.zon" || file.ends_with("/build.zig.zon") {
            scan_zon(&text, &file, &mut out);
        } else if file == "Cargo.toml" || file.ends_with("/Cargo.toml") {
            let root = file == "Cargo.toml";
            scan_cargo(&text, &file, root, metadata_rev, &mut out);
        } else if file.ends_with(".wrap") {
            scan_wrap(&text, &file, &mut out);
        }
    }
    out
}

fn scan_zon(text: &str, file: &str, out: &mut Vec<Pin>) {
    let mut dep = String::new();
    for (idx, line) in text.lines().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('.') {
            if rest.contains('=') && (rest.contains(".{") || rest.contains("= .{")) {
                let name = rest.split('=').next().unwrap_or("").trim();
                if !name.is_empty()
                    && name != "dependencies"
                    && name != "url"
                    && name != "hash"
                    && name != "version"
                    && name != "name"
                    && name != "paths"
                {
                    dep = name.to_string();
                }
            }
        }
        if t.contains(".url") {
            if let Some(url) = quoted(t) {
                let (spec, sha, version) = classify_zig(&url);
                out.push(Pin {
                    file: file.to_string(),
                    line: idx + 1,
                    family: "zig".into(),
                    dep: dep.clone(),
                    spec,
                    sha,
                    version,
                    url,
                    depth: String::new(),
                    diff_files: Vec::new(),
                    section: "dependencies".into(),
                    root: true,
                    raw_hash: String::new(),
                    path_dep: false,
                });
            }
        }
        if t.contains(".hash") {
            if let Some(hash) = quoted(t) {
                if let Some(pin) = out.last_mut() {
                    if pin.file == file && pin.raw_hash.is_empty() {
                        pin.raw_hash = hash;
                    }
                }
            }
        }
    }
}

fn classify_zig(url: &str) -> (String, String, String) {
    let (base, frag) = url
        .split_once('#')
        .map(|(a, b)| (a, b.to_string()))
        .unwrap_or((url, String::new()));
    let reff = base
        .split_once("?ref=")
        .map(|(_, r)| r.to_string())
        .unwrap_or_default();
    if reff == "main" || reff == "develop" {
        return (format!("branch {reff}"), frag, String::new());
    }
    if !reff.is_empty() {
        return (format!("tag {reff}"), frag, reff);
    }
    if !frag.is_empty() {
        return (format!("commit {frag}"), frag, String::new());
    }
    ("commit".into(), String::new(), String::new())
}

fn scan_cargo(text: &str, file: &str, root: bool, metadata_rev: &[String], out: &mut Vec<Pin>) {
    let value: toml::Value = match toml::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    if !metadata_rev.is_empty() {
        scan_metadata(&value, text, file, root, metadata_rev, out);
    }
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = value.get(section).and_then(|v| v.as_table()) {
            for (name, dep) in table {
                push_dep(text, file, root, section, name, dep, out);
            }
        }
    }
    if let Some(patch) = value.get("patch").and_then(|v| v.as_table()) {
        for (reg, body) in patch {
            let Some(table) = body.as_table() else {
                continue;
            };
            let section = format!("patch.{reg}");
            for (name, dep) in table {
                push_dep(text, file, root, &section, name, dep, out);
            }
        }
    }
}

fn push_dep(
    text: &str,
    file: &str,
    root: bool,
    section: &str,
    name: &str,
    dep: &toml::Value,
    out: &mut Vec<Pin>,
) {
    let mut git = String::new();
    let mut rev = String::new();
    let mut branch = String::new();
    let mut version = String::new();
    let mut path_dep = false;
    match dep {
        toml::Value::String(v) => version = v.clone(),
        toml::Value::Table(t) => {
            if let Some(v) = t.get("git").and_then(|x| x.as_str()) {
                git = v.to_string();
            }
            if let Some(v) = t.get("rev").and_then(|x| x.as_str()) {
                rev = v.to_string();
            }
            if let Some(v) = t.get("branch").and_then(|x| x.as_str()) {
                branch = v.to_string();
            }
            if let Some(v) = t.get("version").and_then(|x| x.as_str()) {
                version = v.to_string();
            }
            path_dep = t.contains_key("path") && git.is_empty();
        }
        _ => return,
    }
    let line = line_of(text, &rev)
        .or_else(|| line_of(text, name))
        .unwrap_or(1);
    if !git.is_empty() && !rev.is_empty() {
        out.push(Pin {
            file: file.to_string(),
            line,
            family: "cargo".into(),
            dep: name.to_string(),
            spec: format!("commit {rev}"),
            sha: rev,
            version,
            url: git,
            depth: String::new(),
            diff_files: Vec::new(),
            section: section.to_string(),
            root,
            raw_hash: String::new(),
            path_dep: false,
        });
        return;
    }
    if !git.is_empty() && !branch.is_empty() {
        out.push(Pin {
            file: file.to_string(),
            line,
            family: "cargo".into(),
            dep: name.to_string(),
            spec: format!("branch {branch}"),
            sha: String::new(),
            version,
            url: git,
            depth: String::new(),
            diff_files: Vec::new(),
            section: section.to_string(),
            root,
            raw_hash: String::new(),
            path_dep: false,
        });
        return;
    }
    if path_dep {
        out.push(Pin {
            file: file.to_string(),
            line,
            family: "cargo".into(),
            dep: name.to_string(),
            spec: "path".into(),
            sha: String::new(),
            version,
            url: String::new(),
            depth: String::new(),
            diff_files: Vec::new(),
            section: section.to_string(),
            root,
            raw_hash: String::new(),
            path_dep: true,
        });
        return;
    }
    if !version.is_empty() && exact_version(&version) {
        out.push(Pin {
            file: file.to_string(),
            line,
            family: "registry".into(),
            dep: name.to_string(),
            spec: format!("version {version}"),
            sha: String::new(),
            version,
            url: String::new(),
            depth: String::new(),
            diff_files: Vec::new(),
            section: section.to_string(),
            root,
            raw_hash: String::new(),
            path_dep: false,
        });
    }
}

fn scan_metadata(
    value: &toml::Value,
    text: &str,
    file: &str,
    root: bool,
    keys: &[String],
    out: &mut Vec<Pin>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, child) in table {
        if keys.iter().any(|k| k == key) {
            if let Some(raw) = child.as_str() {
                let sha = raw.trim();
                let hex = sha.chars().all(|c| c.is_ascii_hexdigit());
                if hex && (sha.len() == 8 || sha.len() == 40) {
                    let git_key = key.strip_suffix("-rev").map(|p| format!("{p}-git"));
                    let url = git_key
                        .as_ref()
                        .and_then(|name| table.get(name))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let dep = key.strip_suffix("-rev").unwrap_or(key).to_string();
                    out.push(Pin {
                        file: file.to_string(),
                        line: line_of(text, sha).unwrap_or(1),
                        family: "cargo".into(),
                        dep,
                        spec: format!("commit {sha}"),
                        sha: sha.to_string(),
                        version: String::new(),
                        url,
                        depth: String::new(),
                        diff_files: Vec::new(),
                        section: "metadata".into(),
                        root,
                        raw_hash: String::new(),
                        path_dep: false,
                    });
                }
            }
        }
        scan_metadata(child, text, file, root, keys, out);
    }
}

fn exact_version(v: &str) -> bool {
    let t = v.trim();
    if t.starts_with('^')
        || t.starts_with('~')
        || t.starts_with('>')
        || t.starts_with('<')
        || t.starts_with('*')
    {
        return false;
    }
    true
}

fn scan_wrap(text: &str, file: &str, out: &mut Vec<Pin>) {
    let mut section = String::new();
    let mut url = String::new();
    let mut revision = String::new();
    let mut depth = String::new();
    let mut source_url = String::new();
    let mut source_hash = String::new();
    let mut diff_files = Vec::new();
    let mut line_url = 1usize;
    let flush = |out: &mut Vec<Pin>,
                 section: &str,
                 url: &str,
                 revision: &str,
                 depth: &str,
                 source_url: &str,
                 source_hash: &str,
                 diff_files: &[String],
                 line_url: usize,
                 file: &str| {
        if section == "wrap-git" && !url.is_empty() {
            let spec = if revision.is_empty() {
                "wrap-git".to_string()
            } else if looks_like_sha(revision) {
                format!("commit {revision}")
            } else {
                format!("tag {revision}")
            };
            out.push(Pin {
                file: file.to_string(),
                line: line_url,
                family: "meson".into(),
                dep: wrap_name(file),
                spec,
                sha: if looks_like_sha(revision) {
                    revision.to_string()
                } else {
                    String::new()
                },
                version: if looks_like_sha(revision) {
                    String::new()
                } else {
                    revision.to_string()
                },
                url: url.to_string(),
                depth: depth.to_string(),
                diff_files: diff_files.to_vec(),
                section: "wrap-git".into(),
                root: true,
                raw_hash: String::new(),
                path_dep: false,
            });
        }
        if section == "wrap-file" && !source_url.is_empty() {
            out.push(Pin {
                file: file.to_string(),
                line: line_url,
                family: "meson".into(),
                dep: wrap_name(file),
                spec: "wrap-file".into(),
                sha: String::new(),
                version: String::new(),
                url: source_url.to_string(),
                depth: String::new(),
                diff_files: diff_files.to_vec(),
                section: "wrap-file".into(),
                root: true,
                raw_hash: source_hash.to_string(),
                path_dep: false,
            });
        }
    };
    for (idx, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            flush(
                out,
                &section,
                &url,
                &revision,
                &depth,
                &source_url,
                &source_hash,
                &diff_files,
                line_url,
                file,
            );
            section = t.trim_matches(['[', ']']).to_string();
            url.clear();
            revision.clear();
            depth.clear();
            source_url.clear();
            source_hash.clear();
            diff_files.clear();
            line_url = idx + 1;
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').to_string();
            match key {
                "url" => {
                    url = val;
                    line_url = idx + 1;
                }
                "revision" => revision = val,
                "depth" => depth = val,
                "source_url" => {
                    source_url = val;
                    line_url = idx + 1;
                }
                "source_hash" => source_hash = val,
                "diff_files" => diff_files.push(val),
                _ => {}
            }
        }
    }
    flush(
        out,
        &section,
        &url,
        &revision,
        &depth,
        &source_url,
        &source_hash,
        &diff_files,
        line_url,
        file,
    );
}

fn wrap_name(file: &str) -> String {
    Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("wrap")
        .to_string()
}

fn quoted(line: &str) -> Option<String> {
    let a = line.find('"')?;
    let b = line[a + 1..].find('"')?;
    Some(line[a + 1..a + 1 + b].to_string())
}

fn line_of(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    text.lines().position(|l| l.contains(needle)).map(|i| i + 1)
}

pub fn looks_like_sha(s: &str) -> bool {
    let t = s.trim();
    (t.len() == 40 || t.len() == 7) && t.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn slug(url: &str) -> String {
    let no_git = url.trim_start_matches("git+");
    let no_frag = no_git.split('#').next().unwrap_or(no_git);
    let no_q = no_frag.split('?').next().unwrap_or(no_frag);
    let trimmed = no_q.trim_end_matches('/').trim_end_matches(".git");
    trimmed.rsplit('/').next().unwrap_or("").to_string()
}

pub fn version_key(v: &str) -> String {
    v.trim().trim_start_matches('=').to_string()
}

pub fn read_symbol(text: &str, symbol: &str) -> Option<u64> {
    for line in text.lines() {
        let Some(i) = line.find(symbol) else {
            continue;
        };
        let before = i
            .checked_sub(1)
            .and_then(|n| line.chars().nth(n))
            .unwrap_or(' ');
        let after_idx = i + symbol.len();
        let after = line.chars().nth(after_idx).unwrap_or(' ');
        if before.is_ascii_alphanumeric()
            || before == '_'
            || after.is_ascii_alphanumeric()
            || after == '_'
        {
            continue;
        }
        let rest = &line[after_idx..];
        let digits: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse() {
            return Some(n);
        }
    }
    None
}

pub fn json_field(text: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let i = text.find(&key)?;
    let rest = &text[i + key.len()..];
    let q1 = rest.find('"')?;
    let after = &rest[q1 + 1..];
    let q2 = after.find('"')?;
    Some(after[..q2].to_string())
}
