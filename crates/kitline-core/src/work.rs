use sha2::{Digest, Sha256};

use crate::change::ChangeId;

/// Truncated SHA-256 of the change, the repo, and the target.
///
/// The first 16 bytes are a [`claimdag::WorkId`]. No registry is consulted.
pub fn work_id(change: &ChangeId, repo: &str, target: &str) -> claimdag::WorkId {
    let mut hasher = Sha256::new();
    hasher.update(b"kitline-work-v1\n");
    feed(&mut hasher, change.as_bytes());
    feed(&mut hasher, repo.as_bytes());
    feed(&mut hasher, target.as_bytes());
    let digest = hasher.finalize();
    let mut head = [0u8; 16];
    head.copy_from_slice(&digest[..16]);
    claimdag::WorkId::from_u128(u128::from_be_bytes(head))
}

fn feed(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).expect("length fits u64");
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
}
