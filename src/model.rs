use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct Workspace {
    pub ci: Option<String>,
    pub hash_command: Option<String>,
    pub publish_check: Option<String>,
    pub tickets: Option<String>,
    #[serde(default)]
    pub repos: BTreeMap<String, Repo>,
    #[serde(default)]
    pub node: Vec<Node>,
    #[serde(default)]
    pub publish: Vec<Publish>,
    #[serde(default)]
    pub fix_sha: Vec<String>,
    #[serde(default)]
    pub metadata_rev: Vec<String>,
    #[serde(default)]
    pub claim: Vec<Claim>,
    #[serde(default)]
    pub contract: Vec<Contract>,
    pub records: Option<Records>,
    #[serde(default)]
    pub companion: Vec<Companion>,
    #[serde(default)]
    pub follows: Vec<Follow>,
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub drop_crate: Vec<String>,
    #[serde(default)]
    pub crate_policy: Vec<CratePolicy>,
    #[serde(default)]
    pub manual: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Repo {
    pub path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Node {
    pub id: String,
    pub kind: Option<String>,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub base: Option<String>,
    pub branch: Option<String>,
    pub lease: Option<String>,
    pub lease_matches: Option<String>,
    pub tree: Option<String>,
    pub base_commit: Option<String>,
    pub source: Option<String>,
    pub bundle: Option<String>,
    pub open_pr: Option<bool>,
    pub pr: Option<u64>,
    pub pr_state: Option<String>,
    pub base_pr: Option<u64>,
    pub base_pr_state: Option<String>,
    pub sync: Option<String>,
    pub private: Option<bool>,
    pub land: Option<bool>,
    pub fix: Option<bool>,
    pub merge: Option<String>,
    pub supersedes: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    pub drop: Option<bool>,
    pub gate: Option<String>,
    pub tag: Option<String>,
    #[serde(rename = "crate")]
    pub crate_name: Option<String>,
    pub version: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub generated: Vec<String>,
    #[serde(default)]
    pub companions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Publish {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct Claim {
    pub node: String,
    pub symbol: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Contract {
    pub producer: String,
    pub consumer: String,
    pub compare: String,
    pub producer_path: Option<String>,
    pub producer_symbol: Option<String>,
    pub consumer_path: Option<String>,
    pub consumer_symbol: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Records {
    pub repo: String,
    pub directory: String,
    pub field: String,
    pub rev: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Companion {
    pub node: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Follow {
    pub sha: String,
    pub node: String,
}

#[derive(Debug, Deserialize)]
pub struct CratePolicy {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub upstream: Option<bool>,
    pub keep: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RunFile {
    #[serde(default)]
    pub done: Vec<Done>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Done {
    pub id: String,
    pub landed: Option<bool>,
    pub tree: Option<String>,
    pub tip: Option<String>,
    pub sha: Option<String>,
}
