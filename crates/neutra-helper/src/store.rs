//! Durable compact-base and delta-WAL store.

mod adapters;
mod state;
mod view;

pub(crate) use adapters::*;
pub(crate) use state::DurableStore;

use anyhow::{Context, Result};
use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, DirectorySummary, DirectorySummaryEntry,
    DirectorySummaryOverlay, Query, SearchHit, SearchStats, SpillAccumulator, TreeSummary,
    DELTA_HEADER_BYTES,
};
use std::io::{Read, Write};
mod files;
pub(crate) use files::*;

mod markers;
pub(crate) use markers::*;
#[cfg(target_os = "linux")]
mod maintenance;
