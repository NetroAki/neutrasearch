//! Base-row side of the ordered stream: column scans in growing chunks.
use crate::browse::catalog::Tombstones;
use crate::browse::columns::{BlockReader, Boundary, Columns, Order, Prefilter, Span};
use crate::{FileRecord, Query, SortKey};
use std::{cmp::Ordering, collections::VecDeque, io};

const LARGEST_CHUNK: usize = 1 << 18;

pub(super) fn order_of(sort: SortKey) -> (Order, bool) {
    match sort {
        SortKey::NameAsc => (Order::Name, false),
        SortKey::NameDesc => (Order::Name, true),
        SortKey::PathAsc => (Order::Path, false),
        SortKey::PathDesc => (Order::Path, true),
        SortKey::SizeAsc => (Order::Size, false),
        SortKey::SizeDesc => (Order::Size, true),
        SortKey::MtimeAsc => (Order::Mtime, false),
        SortKey::MtimeDesc | SortKey::Relevance => (Order::Mtime, true),
    }
}

/// Tests start tiny so every refill path runs on small fixtures.
fn first_chunk(hint: usize) -> usize {
    if cfg!(test) {
        1
    } else {
        hint.saturating_mul(2).saturating_add(32).clamp(64, 8192)
    }
}

pub(in crate::browse) struct BaseSide {
    spans: Vec<Span>,
    order: Order,
    descending: bool,
    filter: Prefilter,
    boundary: Option<Boundary>,
    chunk: usize,
    buffer: VecDeque<u32>,
    done: bool,
}

/// Where a scan resumes so that it yields only rows after the anchor.
pub(in crate::browse) fn anchor_boundary(
    cols: &Columns,
    reader: &mut BlockReader<'_>,
    order: Order,
    descending: bool,
    anchor: &FileRecord,
) -> io::Result<Boundary> {
    let bound = |reader: &mut BlockReader<'_>| -> io::Result<u32> {
        if descending {
            reader.path_lower_bound(&anchor.path)
        } else {
            reader.path_lower_bound(&format!("{}\0", anchor.path))
        }
    };
    Ok(match order {
        Order::Size => Boundary {
            key: anchor.disk_bytes().min(crate::browse::columns::SIZE_MAX),
            bound: bound(reader)?,
        },
        Order::Mtime => Boundary {
            key: u64::from(crate::browse::columns::clamp_mtime(anchor.mtime)),
            bound: bound(reader)?,
        },
        Order::Path => Boundary {
            key: u64::from(bound(reader)?),
            bound: 0,
        },
        Order::Name => {
            let position = cols.perm_partition(reader, |row| {
                let found = crate::matcher::cmp_name_ci(row.name(), anchor.name())
                    .then_with(|| row.path.cmp(&anchor.path));
                if descending {
                    found == Ordering::Less
                } else {
                    found != Ordering::Greater
                }
            })?;
            Boundary {
                key: position as u64,
                bound: 0,
            }
        }
    })
}

impl BaseSide {
    pub(in crate::browse) fn new(
        cols: &Columns,
        reader: &mut BlockReader<'_>,
        query: &Query,
        after: Option<&FileRecord>,
        hint: usize,
    ) -> io::Result<Self> {
        let (order, descending) = order_of(query.sort);
        let filter = Prefilter::new(cols, reader, query)?;
        let spans = match order {
            Order::Name => vec![Span::Perm(0, cols.len())],
            _ => filter
                .spans(cols.len())
                .into_iter()
                .map(|(low, high)| Span::Ids(low, high))
                .collect(),
        };
        let boundary = after
            .map(|anchor| anchor_boundary(cols, reader, order, descending, anchor))
            .transpose()?;
        Ok(Self {
            spans,
            order,
            descending,
            filter,
            boundary,
            chunk: first_chunk(hint),
            buffer: VecDeque::new(),
            done: false,
        })
    }

    /// Next live base id in sort order.
    pub(in crate::browse) fn next_id(&mut self, cols: &Columns, dead: &Tombstones) -> Option<u32> {
        if self.buffer.is_empty() && !self.done {
            self.refill(cols, dead);
        }
        self.buffer.pop_front()
    }

    fn refill(&mut self, cols: &Columns, dead: &Tombstones) {
        let filter = &self.filter;
        let accept = |id: u32| filter.accepts(cols, id) && !dead.contains(id);
        let found = cols.top(
            &self.spans,
            self.order,
            self.descending,
            self.chunk,
            self.boundary,
            &accept,
        );
        if found.len() < self.chunk {
            self.done = true;
        }
        if let Some(&(value, id)) = found.last() {
            let packed = if self.descending { !value } else { value };
            let key = (packed >> 32) as u64;
            self.boundary = Some(match self.order {
                Order::Size | Order::Mtime => Boundary {
                    key,
                    bound: if self.descending {
                        id
                    } else {
                        id.saturating_add(1)
                    },
                },
                Order::Path | Order::Name => Boundary {
                    key: if self.descending { key } else { key + 1 },
                    bound: 0,
                },
            });
        }
        self.chunk = (self.chunk * 2).min(LARGEST_CHUNK);
        self.buffer.extend(found.into_iter().map(|(_, id)| id));
    }
}
