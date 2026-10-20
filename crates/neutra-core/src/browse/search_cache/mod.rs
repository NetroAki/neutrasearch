//! Bounded, durable result pages maintained by the catalog's file-event writer.

mod journal;
mod maintenance;
mod ordering;
mod prepare;
mod read;
mod storage;

use crate::Query;
use std::io;

// ponytail: eight 4096-row pages bound event work; a disk candidate index must cover cold and deep queries.
const CAPACITY: usize = 4096;
const MAX_PAGES: usize = 8;

fn signature(query: &Query) -> io::Result<Vec<u8>> {
    let mut normalized = query.clone();
    normalized.limit = 0;
    bincode::serialize(&normalized).map_err(io::Error::other)
}

pub(super) use maintenance::Maintenance;
