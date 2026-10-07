//! Cache-publication events: adopting the compact mmap and building it from
//! the resident index once a scan settles.

use super::events::note;
use super::state::NeutraApp;
use super::types::Event;
 use neutra_core::{CompactIndex, SpillRuns};
use std::time::Instant;

pub(crate) fn adopt_published_index(app: &mut NeutraApp, index: CompactIndex) {
    app.last_generation = index.generation();
    app.compact = Some(std::sync::Arc::new(index));
    if let Err(error) = neutra_core::paths::remember_index_path(&app.cache_path) {
        note(
            app,
            "settings",
            "INDEX LOCATION",
            format!("cannot remember index location: {error}"),
            true,
        );
    }
    // The resident copy stayed searchable throughout the build; reclaim it
    // only after the replacement mmap is verified.
    app.index = neutra_core::Index::default();
    app.building_cache = false;
    app.cache_dirty = false;
    app.last_cache = Instant::now();
    let records = app.index_len();
    note(
        app,
        "cache",
        "COMPACT INDEX",
        "published; idle pages are reclaimable",
        false,
    );
    if let Some(lane) = app.lanes.get_mut("cache") {
        lane.records = records;
    }
    app.requery();
}

 /// Rebuild from the resident index through the spill so even a huge
 /// in-memory result compacts under bounded RAM. The in-RAM builder OOMs
 /// past 25 GiB; this is now the only cache path.
 pub(crate) fn rebuild_cache_index(app: &mut NeutraApp) {
     let runs = match neutra_core::SpillAccumulator::begin(&app.cache_path)
         .and_then(|mut spill| {
             spill.push_batch(app.index.records().to_vec())?;
             spill.finish()
         }) {
         Ok(runs) => runs,
         Err(error) => {
             app.building_cache = false;
             app.cache_dirty = false;
             note(
                 app,
                 "cache",
                 "INDEX BUILD",
                 format!("cannot spill resident index: {error}"),
                 true,
             );
             return;
         }
     };
     build_streaming_cache_index(app, runs);
 }

/// Build a fresh compact base from a finished spill the moment a scan
/// completes. The previous base (if any) stays searchable until the new
/// one publishes; nothing scan-sized ever sits in RAM.
pub(crate) fn build_streaming_cache_index(app: &mut NeutraApp, runs: SpillRuns) {
     let records_count = runs.len();
     let path = app.cache_path.clone();
     let tx = app.tx.clone();
     app.building_cache = true;
    note(
        app,
        "cache",
        "COMPACT INDEX",
        "building compressed search blocks",
        false,
    );
    if let Some(lane) = app.lanes.get_mut("cache") {
        lane.records = records_count;
    }
     std::thread::spawn(move || {
         let built = CompactIndex::rebuild_streamed(runs, &path)
             .and_then(|_| CompactIndex::open_fast(&path));
         match built {
             Ok(compact) => {
                 let _ = tx.send(Event::CompactReady(compact));
             }
             Err(error) => {
                 let _ = tx.send(Event::CompactFailed(error.to_string()));
             }
         }
     });
}
