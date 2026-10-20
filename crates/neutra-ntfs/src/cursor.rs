use super::*;

#[derive(Default)]
pub(crate) struct RunCursor {
    pub(crate) idx: usize,
    pub(crate) logical: u64,
    pub(crate) positioned: bool,
}
impl RunCursor {
    pub(crate) fn read<R: Read + Seek>(
        &mut self,
        r: &mut R,
        runs: &[Run],
        logical: u64,
        out: &mut [u8],
    ) -> Result<()> {
        if !self.positioned || self.logical != logical {
            self.idx = runs
                .iter()
                .position(|x| logical >= x.logical && logical < x.logical + x.len)
                .context("MFT logical hole")?;
            let run = runs[self.idx];
            r.seek(SeekFrom::Start(run.physical + logical - run.logical))?;
            self.logical = logical;
            self.positioned = true;
        }
        let mut done = 0usize;
        while done < out.len() {
            let run = runs.get(self.idx).context("MFT run exhausted")?;
            let within = self.logical - run.logical;
            if within >= run.len {
                self.idx += 1;
                self.positioned = false;
                continue;
            }
            let n = ((run.len - within) as usize).min(out.len() - done);
            r.read_exact(&mut out[done..done + n])?;
            done += n;
            self.logical += n as u64;
            if self.logical == run.logical + run.len {
                self.idx += 1;
                if let Some(next) = runs.get(self.idx) {
                    r.seek(SeekFrom::Start(next.physical))?;
                    self.positioned = true;
                }
            }
        }
        Ok(())
    }
}
