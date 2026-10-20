//! Live-row side of the ordered stream: keyset pages from the small table.
use crate::browse::query::{cursor_clause, filters, ordering, tie_path_direction};
use crate::browse::sql_error;
use crate::{FileRecord, Query, SortKey};
use rusqlite::{types::Value, Connection};
use std::{collections::VecDeque, io};

const PAGE: usize = 512;

pub(in crate::browse) struct LiveSide {
    sort: SortKey,
    index: &'static str,
    order: String,
    conditions: String,
    values: Vec<Value>,
    anchor: Option<FileRecord>,
    buffer: VecDeque<FileRecord>,
    done: bool,
}

impl LiveSide {
    pub(in crate::browse) fn new(query: &Query, after: Option<&FileRecord>) -> Self {
        let (primary, index) = ordering(query.sort);
        let order = if matches!(
            query.sort,
            SortKey::NameAsc | SortKey::NameDesc | SortKey::PathAsc | SortKey::PathDesc
        ) {
            primary.to_owned()
        } else {
            format!("{primary},path {}", tie_path_direction(query.sort))
        };
        let (conditions, values) = filters(query);
        Self {
            sort: query.sort,
            index,
            order,
            conditions,
            values,
            anchor: after.cloned(),
            buffer: VecDeque::new(),
            done: false,
        }
    }

    pub(in crate::browse) fn next_record(
        &mut self,
        db: &Connection,
    ) -> io::Result<Option<FileRecord>> {
        if self.buffer.is_empty() && !self.done {
            self.refill(db)?;
        }
        Ok(self.buffer.pop_front())
    }

    fn refill(&mut self, db: &Connection) -> io::Result<()> {
        let mut conditions = self.conditions.clone();
        let mut values = self.values.clone();
        if let Some(anchor) = &self.anchor {
            conditions.push_str(&format!(
                " AND {}",
                cursor_clause(self.sort, anchor, &mut values)
            ));
        }
        values.push(Value::Integer(PAGE as i64));
        let sql = format!(
            "SELECT record FROM entries INDEXED BY {} WHERE {conditions} ORDER BY {} LIMIT ?",
            self.index, self.order
        );
        let mut statement = db.prepare(&sql).map_err(sql_error)?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(values))
            .map_err(sql_error)?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let bytes: Vec<u8> = row.get(0).map_err(sql_error)?;
            let record: FileRecord = bincode::deserialize(&bytes).map_err(io::Error::other)?;
            self.anchor = Some(record.clone());
            self.buffer.push_back(record);
            count += 1;
        }
        self.done = count < PAGE;
        Ok(())
    }
}
