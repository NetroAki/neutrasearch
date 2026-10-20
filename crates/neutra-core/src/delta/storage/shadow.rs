//! In-memory membership filter for the overlay's paths. Searches ask "is this
//! base record shadowed?" for millions of matches; answering from SQLite under
//! one mutex made broad queries minutes long. The filter answers "definitely
//! shadowed" or "shadowed" in nanoseconds. Entries are 128-bit hashes, so a
//! false answer needs a collision of about 2^-128 per pair.

use super::*;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default)]
pub(super) struct Passthrough(u64);

impl Hasher for Passthrough {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 << 8) | u64::from(*byte);
        }
    }
    fn write_u128(&mut self, value: u128) {
        self.0 = (value >> 64) as u64 ^ value as u64;
    }
}

type HashSet64 = HashSet<u128, BuildHasherDefault<Passthrough>>;

fn hash(bytes: &[u8]) -> u128 {
    (u128::from(mix(bytes, 0x9e37_79b9_7f4a_7c15)) << 64)
        | u128::from(mix(bytes, 0xc2b2_ae3d_27d4_eb4f))
}

fn mix(bytes: &[u8], seed: u64) -> u64 {
    let mut state = seed ^ bytes.len() as u64;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        state =
            (state.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    state ^= state >> 33;
    state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
    state ^ (state >> 33)
}

#[derive(Clone)]
pub(in crate::delta) struct ShadowFilter {
    exact: HashSet64,
    prefixes: HashSet64,
    /// Every `SAMPLE_EVERY`th path in key order, to split scans evenly.
    samples: Vec<String>,
}

const SAMPLE_EVERY: usize = 16_384;

impl ShadowFilter {
    pub(super) fn load(db: &Connection) -> io::Result<Self> {
        let mut filter = Self {
            exact: HashSet64::default(),
            prefixes: HashSet64::default(),
            samples: Vec::new(),
        };
        for (table, set) in [
            ("changes", &mut filter.exact),
            ("prefixes", &mut filter.prefixes),
        ] {
            let mut seen = 0usize;
            let mut statement = db
                .prepare(&format!("SELECT path FROM {table}"))
                .map_err(sql_error)?;
            let mut rows = statement.query([]).map_err(sql_error)?;
            while let Some(row) = rows.next().map_err(sql_error)? {
                let path = row
                    .get_ref(0)
                    .map_err(sql_error)?
                    .as_str()
                    .map_err(sql_error)?;
                set.insert(hash(path.as_bytes()));
                if table == "changes" && seen.is_multiple_of(SAMPLE_EVERY) {
                    filter.samples.push(path.to_owned());
                }
                seen += 1;
            }
        }
        Ok(filter)
    }

    /// Up to `parts - 1` ascending keys that cut the table into similar parts.
    pub(super) fn split_points(&self, parts: usize) -> Vec<String> {
        let mut samples = self.samples.clone();
        samples.sort_unstable();
        let parts = parts.min(samples.len()).max(1);
        let mut points: Vec<String> = (1..parts)
            .map(|k| samples[k * samples.len() / parts].clone())
            .collect();
        points.dedup();
        points
    }

    pub(super) fn insert(&mut self, path: &str) {
        self.exact.insert(hash(path.as_bytes()));
    }

    /// Distinct paths held, which is the overlay's row count.
    pub(super) fn len(&self) -> usize {
        self.exact.len() + self.prefixes.len()
    }

    /// Whether the overlay holds the path or a removed ancestor of it.
    pub(in crate::delta) fn shadows(&self, path: &str) -> bool {
        self.exact.contains(&hash(path.as_bytes()))
            || (!self.prefixes.is_empty()
                && prefixes::ancestors(path).any(|a| self.prefixes.contains(&hash(a.as_bytes()))))
    }
}
