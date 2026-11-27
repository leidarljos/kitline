#[path = "common/mod.rs"]
mod common;

use kitline_core::{identical, kit_make, kit_verify, revision};

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
    common::commit(
        &repo.path,
        "first\n",
        "2020-01-02T00:00:00+0000",
        "2020-01-02T00:00:00+0000",
    );
    common::write(&repo.path, "f", "a\nb\nc\n");
    common::commit(
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
    common::commit(
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
