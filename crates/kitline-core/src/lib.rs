//! Change identity, revision ids, and kit deeds.
//!
//! A change is a [`ChangeId`] carried in a `Kitline-Change` trailer.
//! A [`RevId`] names the series. The base commit is recorded on
//! [`Revision`] and is not part of that hash.

mod change;
mod error;
mod forge;
mod gitcmd;
mod kit;
mod rev;
mod trailer;
mod work;

pub use change::{ChangeId, ObjectId, RevId};
pub use error::{Error, Result};
pub use forge::{LandedSha, ReviewHandle, ReviewedHead, SetId};
pub use kit::{kit_make, kit_verify, Made};
pub use rev::{identical, patch_equal, revision, tree_equal, Revision};
pub use trailer::{mint, read, stamp, Stamped};
pub use work::work_id;
