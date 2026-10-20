use super::SLOTS;
use rusqlite::{params, Connection};
use std::io;

pub(in crate::browse) struct Counts {
    pub(super) slots: [i64; SLOTS],
}

impl Default for Counts {
    fn default() -> Self {
        Self { slots: [0; SLOTS] }
    }
}

impl Counts {
    pub(in crate::browse) fn add_name(&mut self, name: &str, weight: i64) {
        let mut seen = [0_u64; SLOTS.div_ceil(64)];
        let mut previous = None;
        for byte in name.bytes() {
            let Some(code) = ascii_alnum(byte) else {
                previous = None;
                continue;
            };
            let single = code;
            mark(&mut self.slots, &mut seen, single, weight);
            if let Some(left) = previous {
                mark(&mut self.slots, &mut seen, 36 + left * 36 + code, weight);
            }
            previous = Some(code);
        }
    }

    pub(in crate::browse) fn publish(&self, db: &Connection, unix_paths: bool) -> io::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS short_counts(code INTEGER PRIMARY KEY, matched INTEGER NOT NULL CHECK(matched>=0));\
             CREATE TABLE IF NOT EXISTS short_ready(version INTEGER NOT NULL, unix_paths INTEGER NOT NULL);",
        ).map_err(super::super::sql_error)?;
        db.execute("DELETE FROM short_counts", [])
            .map_err(super::super::sql_error)?;
        {
            let mut insert = db
                .prepare_cached("INSERT INTO short_counts(code,matched) VALUES(?1,?2)")
                .map_err(super::super::sql_error)?;
            for (code, count) in self.slots.iter().enumerate() {
                insert
                    .execute(params![code as i64, *count])
                    .map_err(super::super::sql_error)?;
            }
        }
        db.execute("DELETE FROM short_ready", [])
            .map_err(super::super::sql_error)?;
        db.execute(
            "INSERT INTO short_ready(version,unix_paths) VALUES(?1,?2)",
            params![super::VERSION, i64::from(unix_paths)],
        )
        .map_err(super::super::sql_error)?;
        Ok(())
    }
}

pub(in crate::browse) fn key(term: &str) -> Option<usize> {
    let bytes = term.as_bytes();
    match bytes {
        [one] => ascii_alnum(*one),
        [left, right] => Some(36 + ascii_alnum(*left)? * 36 + ascii_alnum(*right)?),
        _ => None,
    }
}

fn ascii_alnum(byte: u8) -> Option<usize> {
    match byte.to_ascii_lowercase() {
        b'a'..=b'z' => Some((byte.to_ascii_lowercase() - b'a') as usize),
        b'0'..=b'9' => Some((byte - b'0') as usize + 26),
        _ => None,
    }
}

fn mark(slots: &mut [i64; SLOTS], seen: &mut [u64; SLOTS.div_ceil(64)], code: usize, weight: i64) {
    let bit = 1_u64 << (code % 64);
    let word = &mut seen[code / 64];
    if *word & bit == 0 {
        *word |= bit;
        slots[code] += weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_unique_ascii_terms_and_respects_boundaries() {
        let mut counts = Counts::default();
        counts.add_name("Aaa aa-11 ébc", 1);
        assert_eq!(counts.slots[key("a").unwrap()], 1);
        assert_eq!(counts.slots[key("aa").unwrap()], 1);
        assert_eq!(counts.slots[key("11").unwrap()], 1);
        assert_eq!(key("é"), None);
    }
}
