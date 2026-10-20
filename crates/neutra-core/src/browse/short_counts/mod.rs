mod maintenance;
mod tally;

pub(in crate::browse) use maintenance::{matched, ready, remove_prefix, update};
pub(in crate::browse) use tally::{key, Counts};

pub(super) const VERSION: i64 = 1;
pub(super) const SLOTS: usize = 1332;
