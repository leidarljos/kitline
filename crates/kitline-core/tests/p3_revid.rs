#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use kitline_core::{identical, patch_equal, revision, tree_equal, ObjectId};

fn series(parent: &Path, base_msg: &str, change_msg: &str) -> (common::Repo, String, String) {
    let repo = common::repo(parent);
    common::write(&repo.path, "f", "a\n");
    common::commit(
        &repo.path,
        base_msg,
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::write(&repo.path, "f", "a\nb\n");
    common::commit(
        &repo.path,
        change_msg,
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    (repo, base, tip)
}

fn tree(repo: &Path) -> ObjectId {
    ObjectId::parse(&common::git(repo, &["rev-parse", "HEAD^{tree}"], &[])).unwrap()
}

#[test]
fn one_message_byte_changes_rev_and_moved_base_stays_patch_equal() {
    let root = common::Tmp::new();
    let (repo_a, base_a, tip_a) = series(root.path(), "base\n", "change the file\n");
    let (repo_b, base_b, tip_b) = series(&root.path().join("byte"), "base\n", "change the filf\n");
    let (repo_c, base_c, tip_c) = series(
        &root.path().join("moved"),
        "other base\n",
        "change the file\n",
    );

    let rev_a = revision(&repo_a.path, &base_a, &tip_a).unwrap();
    let rev_b = revision(&repo_b.path, &base_b, &tip_b).unwrap();
    let rev_c = revision(&repo_c.path, &base_c, &tip_c).unwrap();

    assert_ne!(rev_a.rev, rev_b.rev, "one message byte changes the rev");
    assert!(!identical(&rev_a, &rev_b));
    assert!(!patch_equal(&rev_a, &rev_b));
    assert!(tree_equal(&tree(&repo_a.path), &tree(&repo_b.path)));

    assert_eq!(rev_a.rev, rev_c.rev);
    assert_ne!(rev_a.base, rev_c.base);
    assert!(patch_equal(&rev_a, &rev_c));
    assert!(!identical(&rev_a, &rev_c));
}
