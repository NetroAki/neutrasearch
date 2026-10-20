use super::*;

type EncodedBlock = (Vec<u8>, Vec<u32>, u16);

pub(super) fn blocks(records: &[FileRecord], order: &[u32]) -> io::Result<Vec<EncodedBlock>> {
    order
        .par_chunks(BLOCK_RECORDS)
        .map(|ids| {
            let mut grams = Vec::new();
            {
                let mut set = HashSet::new();
                for id in ids {
                    collect_trigrams(&records[*id as usize].path, &mut set);
                }
                grams.extend(set);
            }
            let refs = ids
                .iter()
                .map(|id| &records[*id as usize])
                .collect::<Vec<_>>();
            let raw = bincode::serialize(&refs).map_err(binerr)?;
            let compressed = zstd::bulk::compress(&raw, 1)?;
            grams.shrink_to_fit();
            Ok((compressed, grams, ids.len() as u16))
        })
        .collect::<io::Result<Vec<_>>>()
}
