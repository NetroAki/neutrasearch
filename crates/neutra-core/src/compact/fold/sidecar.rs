//! Directory-summary sidecar fed from the folded record stream. The feed
//! needs (source, path) order; one source at a time is enough for the common
//! single-source index, and interleaved sources fall back to a full rebuild.

use crate::compact_summary::{begin_sidecar, SidecarSink, SummaryFeed};
use crate::FileRecord;
use std::io;
use std::path::Path;

pub(super) struct Feeds {
    sink: SidecarSink,
    feed: SummaryFeed,
    source: Option<u32>,
}

impl Feeds {
    pub(super) fn begin(staged: &Path, generation: u64) -> io::Result<Self> {
        Ok(Self {
            sink: begin_sidecar(staged, generation)?,
            feed: SummaryFeed::new(),
            source: None,
        })
    }

    pub(super) fn push(&mut self, record: &FileRecord) -> io::Result<()> {
        match self.source {
            Some(source) if source == record.source => {}
            Some(source) if record.source < source => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "sources interleave in path order",
                ));
            }
            Some(_) => {
                let finished = std::mem::replace(&mut self.feed, SummaryFeed::new());
                finished.finish(&mut |entry| self.sink.push(entry))?;
                self.source = Some(record.source);
            }
            None => self.source = Some(record.source),
        }
        let sink = &mut self.sink;
        self.feed.push(record, &mut |entry| sink.push(entry))
    }

    pub(super) fn finish(self) -> io::Result<()> {
        let Self { mut sink, feed, .. } = self;
        feed.finish(&mut |entry| sink.push(entry))?;
        sink.finish()
    }
}
