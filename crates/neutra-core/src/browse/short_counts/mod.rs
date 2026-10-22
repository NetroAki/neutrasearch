mod batch;
mod maintenance;
mod tally;

pub(in crate::browse) use batch::Batch;
pub(in crate::browse) use maintenance::{matched, ready, remove_prefix};
pub(in crate::browse) use tally::{key, Counts};

pub(super) const VERSION: i64 = 1;
pub(super) const SLOTS: usize = 1332;
