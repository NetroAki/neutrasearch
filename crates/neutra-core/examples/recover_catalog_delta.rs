use neutra_core::BrowserIndex;
use std::{io, path::Path};

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [index, output] = args.as_slice() else {
        return Err(io::Error::other(
            "usage: recover_catalog_delta INDEX.nsx NEW.delta",
        ));
    };
    BrowserIndex::recover_delta(Path::new(index), Path::new(output))
}
