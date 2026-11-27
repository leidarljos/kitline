#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use kitline_core::{
    mint, read, stamp, work_id, ChangeId, Error, LandedSha, ReviewHandle, ReviewedHead, SetId,
};

fn read_at(repo: &Path, spec: &str) -> ChangeId {
    let raw = common::git_out(repo, &["cat-file", "commit", spec]);
    let text = String::from_utf8(raw).unwrap();
    read(&text).unwrap().unwrap()
}

fn commit_at(repo: &Path, message: &str, when: &str) {
    common::commit(repo, message, "2020-01-02T00:00:00+0000", when);
}

#[test]
fn trailer_survives_rewrite_and_header_disagreement_is_refused() {
    let zero = ChangeId::parse(&"z".repeat(32)).unwrap();
    assert_eq!(zero.as_bytes(), &[0u8; 16]);
    let sample = ChangeId::parse("zyxwvutsrqponmlkzyxwvutsrqponmlk").unwrap();
    assert_eq!(&sample.as_bytes()[..8], b"\x01\x23\x45\x67\x89\xab\xcd\xef");
    assert!(ChangeId::parse(&"k".repeat(31)).is_err());
    assert!(ChangeId::parse(&format!("j{}", "k".repeat(31))).is_err());

    let review = ReviewHandle::parse("review/12").unwrap();
    assert_eq!(review.to_string(), "review/12");
    assert!(ReviewHandle::parse("12").is_err());
    let set = SetId::parse("set/3").unwrap();
    assert_eq!(set.to_string(), "set/3");
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let head = ReviewedHead::parse(sha).unwrap();
    let landed = LandedSha::parse(sha).unwrap();
    assert_eq!(head.to_string(), sha);
    assert_eq!(landed.to_string(), sha);
    assert!(ReviewedHead::parse("review/12").is_err());
    assert!(LandedSha::parse(&"z".repeat(32)).is_err());

    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::write(&repo.path, "base.txt", "base\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);

    let id = mint();
    let stamped = stamp("add the change\n", &id).unwrap();
    assert!(stamped.added);
    assert_eq!(stamped.id, id);
    let again = stamp(&stamped.message, &mint()).unwrap();
    assert!(!again.added);
    assert_eq!(again.id, id);
    assert_eq!(again.message, stamped.message);
    assert_eq!(read(&stamped.message).unwrap(), Some(id));

    let w1 = work_id(&id, "repo", "main");
    let w2 = work_id(&id, "repo", "main");
    assert_eq!(w1, w2);
    assert_ne!(w1, work_id(&id, "other", "main"));
    assert_eq!(w1.to_hex().len(), 32);

    common::write(&repo.path, "change.txt", "change\n");
    commit_at(&repo.path, &stamped.message, "2020-01-02T00:00:01+0000");
    let sha_commit = common::head(&repo.path);
    assert_eq!(read_at(&repo.path, "HEAD"), id);

    common::git(
        &repo.path,
        &["commit", "--amend", "--no-edit"],
        &[("GIT_COMMITTER_DATE", "2020-01-03T00:00:00+0000")],
    );
    let sha_amend = common::head(&repo.path);
    assert_ne!(sha_amend, sha_commit);
    assert_eq!(read_at(&repo.path, "HEAD"), id);

    common::git(&repo.path, &["branch", "side", &base], &[]);
    common::git(&repo.path, &["checkout", "-q", "side"], &[]);
    common::write(&repo.path, "other.txt", "other\n");
    common::commit(
        &repo.path,
        "side base\n",
        "2020-01-04T00:00:00+0000",
        "2020-01-04T00:00:00+0000",
    );
    common::git(&repo.path, &["checkout", "-q", "main"], &[]);
    common::git(
        &repo.path,
        &["rebase", "side"],
        &[("GIT_COMMITTER_DATE", "2020-01-05T00:00:00+0000")],
    );
    let sha_rebase = common::head(&repo.path);
    assert_ne!(sha_rebase, sha_amend);
    assert_eq!(read_at(&repo.path, "HEAD"), id);

    common::git(&repo.path, &["checkout", "-q", "-B", "pick", &base], &[]);
    common::git(
        &repo.path,
        &["cherry-pick", &sha_rebase],
        &[("GIT_COMMITTER_DATE", "2020-01-06T00:00:00+0000")],
    );
    let sha_pick = common::head(&repo.path);
    assert_ne!(sha_pick, sha_rebase);
    assert_eq!(read_at(&repo.path, "HEAD"), id);

    let patch = root.path().join("one.patch");
    let bytes = common::git_out(&repo.path, &["format-patch", "-1", "--stdout", "HEAD"]);
    std::fs::write(&patch, &bytes).unwrap();
    common::git(&repo.path, &["checkout", "-q", "-B", "ammed", &base], &[]);
    common::git(
        &repo.path,
        &["am", patch.to_str().unwrap()],
        &[("GIT_COMMITTER_DATE", "2020-01-07T00:00:00+0000")],
    );
    let sha_am = common::head(&repo.path);
    assert_ne!(sha_am, sha_pick);
    assert_eq!(read_at(&repo.path, "HEAD"), id);

    let bare = root.path().join("bare.git");
    common::git_here(&["init", "--bare", "-q", "-b", "main", bare.to_str().unwrap()]);
    common::git(
        &repo.path,
        &["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        &[],
    );
    let fetched = root.path().join("fetched");
    common::git_here(&[
        "clone",
        "-q",
        bare.to_str().unwrap(),
        fetched.to_str().unwrap(),
    ]);
    let fetched_before = common::git(&fetched, &["rev-parse", "HEAD"], &[]);
    assert_eq!(fetched_before, sha_am);

    common::git(
        &repo.path,
        &["commit", "--amend", "--no-edit"],
        &[("GIT_COMMITTER_DATE", "2020-01-08T00:00:00+0000")],
    );
    let sha_force = common::head(&repo.path);
    assert_ne!(sha_force, sha_am);
    assert_eq!(read_at(&repo.path, "HEAD"), id);
    common::git(
        &repo.path,
        &[
            "push",
            "--force",
            bare.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ],
        &[],
    );
    common::git(&fetched, &["fetch", "-q", "origin"], &[]);
    let sha_fetch = common::git(&fetched, &["rev-parse", "origin/main"], &[]);
    assert_eq!(sha_fetch, sha_force);
    assert_ne!(sha_fetch, fetched_before);
    assert_eq!(read_at(&fetched, "origin/main"), id);

    let other = mint();
    let tree = common::git(&repo.path, &["rev-parse", "HEAD^{tree}"], &[]);
    let parent = common::git(&repo.path, &["rev-parse", "HEAD^"], &[]);
    let raw = format!(
        "tree {tree}\nparent {parent}\nauthor Kit Tester <kit@example.com> 0 +0000\ncommitter Kit Tester <kit@example.com> 0 +0000\nchange-id {other}\n\n{msg}",
        msg = stamped.message
    );
    match read(&raw) {
        Err(Error::Disagree { header, trailer }) => {
            assert_eq!(header, other.to_string());
            assert_eq!(trailer, id.to_string());
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    let agreed = format!(
        "tree {tree}\nparent {parent}\nauthor Kit Tester <kit@example.com> 0 +0000\ncommitter Kit Tester <kit@example.com> 0 +0000\nchange-id {id}\n\n{msg}",
        msg = stamped.message
    );
    assert_eq!(read(&agreed).unwrap(), Some(id));
    let header_only = format!(
        "tree {tree}\nparent {parent}\nauthor Kit Tester <kit@example.com> 0 +0000\ncommitter Kit Tester <kit@example.com> 0 +0000\nchange-id {id}\n\nadd the change\n"
    );
    assert_eq!(read(&header_only).unwrap(), Some(id));
}
