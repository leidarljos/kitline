#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use kitline_core::{identical, kit_make, kit_verify, mint, revision, stamp, Error};

fn commit_with_id(repo: &Path, message: &str, author: &str, committer: &str) {
    let id = mint();
    let stamped = stamp(message, &id).unwrap();
    common::commit(repo, &stamped.message, author, committer);
}

#[test]
fn bundle_and_mbox_share_one_rev_and_verify_rejects_a_flipped_byte() {
    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::write(&repo.path, "f", "a\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::write(&repo.path, "f", "a\nb\n");
    commit_with_id(
        &repo.path,
        "first\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    common::write(&repo.path, "f", "a\nb\nc\n");
    commit_with_id(
        &repo.path,
        "second\n",
        "2020-01-03T00:00:00+0000",
        "2020-01-03T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    let rev = revision(&repo.path, &base, &tip).unwrap();

    let out = root.path().join("kit");
    let store = root.path().join("store");
    let made = kit_make(&repo.path, &format!("{base}..{tip}"), &out, &store, None).unwrap();
    assert_eq!(made.revision.rev, rev.rev);
    assert_eq!(made.revision.base.as_str(), base);

    let checked = kit_verify(&repo.path, &out, &store).unwrap();
    assert!(identical(&made.revision, &checked));

    let out_b = root.path().join("kit-other");
    let again = kit_make(&repo.path, &format!("{base}..{tip}"), &out_b, &store, None).unwrap();
    assert_eq!(again.deed_id, made.deed_id);

    let fresh = root.path().join("fresh");
    common::git_here(&[
        "clone",
        "-q",
        "--local",
        repo.path.to_str().unwrap(),
        fresh.to_str().unwrap(),
    ]);
    common::git(&fresh, &["config", "user.name", "Kit Tester"], &[]);
    common::git(&fresh, &["config", "user.email", "kit@example.com"], &[]);
    common::git(&fresh, &["checkout", "-q", "--detach", &base], &[]);
    common::git(
        &fresh,
        &["am", out.join("series.mbox").to_str().unwrap()],
        &[("GIT_COMMITTER_DATE", "2020-02-01T00:00:00+0000")],
    );
    let applied = revision(&fresh, &base, "HEAD").unwrap();
    assert_eq!(applied.rev, rev.rev);
    assert_ne!(common::head(&fresh), tip);

    common::write(&repo.path, "f", "a\nb\nc\nd\n");
    commit_with_id(
        &repo.path,
        "third\n",
        "2020-01-04T00:00:00+0000",
        "2020-01-04T00:00:00+0000",
    );
    let tip2 = common::head(&repo.path);
    let out2 = root.path().join("kit2");
    let made2 = kit_make(
        &repo.path,
        &format!("{base}..{tip2}"),
        &out2,
        &store,
        Some(&made.deed_id),
    )
    .unwrap();
    assert_ne!(made2.revision.rev, made.revision.rev);
    let url = format!("file://{}", store.canonicalize().unwrap().display());
    let mut client = deedar::Client::open(&url).unwrap();
    let current = client.current(&made.deed_id).unwrap();
    assert_eq!(current.id, made2.deed_id);
    assert!(identical(
        &made2.revision,
        &kit_verify(&repo.path, &out2, &store).unwrap()
    ));

    let mbox_path = out.join("series.mbox");
    let mut bytes = std::fs::read(&mbox_path).unwrap();
    let i = bytes.len() / 2;
    bytes[i] ^= 0x01;
    std::fs::write(&mbox_path, bytes).unwrap();
    assert!(kit_verify(&repo.path, &out, &store).is_err());
}

#[test]
fn kit_make_refuses_a_symlink_outside_the_kit() {
    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::write(&repo.path, "f", "a\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::write(&repo.path, "f", "a\nb\n");
    commit_with_id(
        &repo.path,
        "first\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    let outside = root.path().join("outside.txt");
    std::fs::write(&outside, b"keep-these-bytes\n").unwrap();
    let out = root.path().join("kit");
    std::fs::create_dir_all(&out).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("series.mbox")).unwrap();
    let made = kit_make(
        &repo.path,
        &format!("{base}..{tip}"),
        &out,
        &root.path().join("store"),
        None,
    );
    assert!(made.is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep-these-bytes\n");
}

#[test]
fn verify_refuses_a_divider_message_that_am_would_drop() {
    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::write(&repo.path, "f", "a\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::write(&repo.path, "f", "a\nb\n");
    commit_with_id(
        &repo.path,
        "subject\n\nbody\n---\nstill the message\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    let out = root.path().join("kit");
    let store = root.path().join("store");
    kit_make(&repo.path, &format!("{base}..{tip}"), &out, &store, None).unwrap();
    assert!(kit_verify(&repo.path, &out, &store).is_err());
}

#[test]
fn verify_accepts_a_rename_when_diff_renames_is_false() {
    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::git(&repo.path, &["config", "diff.renames", "false"], &[]);
    common::git(&repo.path, &["config", "format.headers", "X-Kit: 1"], &[]);
    common::write(&repo.path, "old.txt", "same\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::git(&repo.path, &["mv", "old.txt", "new.txt"], &[]);
    commit_with_id(
        &repo.path,
        "rename\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    let out = root.path().join("kit");
    let store = root.path().join("store");
    let made = kit_make(&repo.path, &format!("{base}..{tip}"), &out, &store, None).unwrap();
    let checked = kit_verify(&repo.path, &out, &store).unwrap();
    assert!(identical(&made.revision, &checked));
}

#[test]
fn kit_make_refuses_a_commit_with_no_change_id() {
    let root = common::Tmp::new();
    let repo = common::repo(root.path());
    common::write(&repo.path, "f", "a\n");
    common::commit(
        &repo.path,
        "base\n",
        "2020-01-01T00:00:00+0000",
        "2020-01-01T00:00:00+0000",
    );
    let base = common::head(&repo.path);
    common::write(&repo.path, "f", "a\nb\n");
    common::commit(
        &repo.path,
        "unstamped subject\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    let tip = common::head(&repo.path);
    let err = kit_make(
        &repo.path,
        &format!("{base}..{tip}"),
        &root.path().join("kit"),
        &root.path().join("store"),
        None,
    )
    .unwrap_err();
    match err {
        Error::Kit(text) => {
            assert!(text.contains(&tip), "{text}");
            assert!(text.contains("unstamped subject"), "{text}");
        }
        other => panic!("expected a kit refusal, got {other:?}"),
    }
}
