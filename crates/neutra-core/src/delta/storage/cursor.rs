use super::*;

/// Verified WAL position the overlay embodies. Committed in the same SQLite
/// transaction as the frames it describes, so a crash leaves either the old
/// cursor (the tail replays on next open) or the new cursor with the
/// complete resolved state. The WAL stays authoritative in all cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::delta) struct Cursor {
    pub generation: u64,
    pub wal_bytes: u64,
    pub legacy_frames: bool,
    /// Modification time of the WAL at the verified cursor, in nanoseconds
    /// since the Unix epoch. Detects same-length in-place edits.
    pub modified_ns: u128,
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
}

impl DeltaStore {
    pub(in crate::delta) fn cursor(&self) -> io::Result<Option<Cursor>> {
        let db = self.db()?;
        let get = |key: &str| -> io::Result<Option<String>> {
            db.query_row("SELECT v FROM meta WHERE k=?1", [key], |row| row.get(0))
                .optional()
                .map_err(sql_error)
        };
        let (Some(generation), Some(wal_bytes), Some(legacy)) =
            (get("generation")?, get("wal_bytes")?, get("legacy_frames")?)
        else {
            return Ok(None);
        };
        let Some(modified_ns) = get("modified_ns")? else {
            return Ok(None);
        };
        #[cfg(unix)]
        let (Some(dev), Some(ino)) = (get("dev")?, get("ino")?) else {
            return Ok(None);
        };
        let parse = |key: &str, v: &str| {
            v.parse::<u64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("delta overlay cursor {key} corrupt"),
                )
            })
        };
        Ok(Some(Cursor {
            generation: parse("generation", &generation)?,
            wal_bytes: parse("wal_bytes", &wal_bytes)?,
            legacy_frames: legacy != "0",
            modified_ns: modified_ns.parse::<u128>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "delta overlay cursor modified_ns corrupt",
                )
            })?,
            #[cfg(unix)]
            dev: parse("dev", &dev)?,
            #[cfg(unix)]
            ino: parse("ino", &ino)?,
        }))
    }

    pub(in crate::delta) fn set_cursor(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.commit().map_err(sql_error)
    }

    #[cfg(unix)]
    pub(in crate::delta) fn set_checkpoint_cursor(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta VALUES('checkpoint_bytes',?1)",
            [cursor.wal_bytes.to_string()],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)
    }

    pub(in crate::delta) fn store_cursor_tx(
        tx: &rusqlite::Transaction,
        cursor: &Cursor,
    ) -> io::Result<()> {
        let legacy = if cursor.legacy_frames { "1" } else { "0" };
        tx.execute("INSERT INTO meta(k,v) VALUES('generation',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.generation.to_string()]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('wal_bytes',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.wal_bytes.to_string()]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('legacy_frames',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [legacy]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('modified_ns',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.modified_ns.to_string()]).map_err(sql_error)?;
        #[cfg(unix)]
        {
            tx.execute(
                "INSERT INTO meta(k,v) VALUES('dev',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [cursor.dev.to_string()],
            )
            .map_err(sql_error)?;
            tx.execute(
                "INSERT INTO meta(k,v) VALUES('ino',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [cursor.ino.to_string()],
            )
            .map_err(sql_error)?;
        }
        Ok(())
    }
}
