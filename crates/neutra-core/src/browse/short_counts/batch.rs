use super::{maintenance, Counts};
use crate::{browse::sql_error, FileRecord};
use rusqlite::Connection;
use std::io;

#[derive(Default)]
pub(in crate::browse) struct Batch {
    counts: Counts,
    non_unix: bool,
}

impl Batch {
    pub(in crate::browse) fn change(&mut self, old: Option<&FileRecord>, new: Option<&FileRecord>) {
        if old.map(FileRecord::name) != new.map(FileRecord::name) {
            if let Some(record) = old {
                self.counts.add_name(record.name(), -1);
            }
            if let Some(record) = new {
                self.counts.add_name(record.name(), 1);
            }
        }
        self.non_unix |= new.is_some_and(|record| !record.path.starts_with('/'));
    }

    pub(in crate::browse) fn finish(&self, db: &Connection) -> io::Result<()> {
        maintenance::apply(db, &self.counts.slots)?;
        if self.non_unix {
            db.execute("UPDATE short_ready SET unix_paths=0", [])
                .map_err(sql_error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
