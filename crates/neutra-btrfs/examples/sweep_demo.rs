//! Manual check of the incremental sweep against a real subvolume:
//! sweep_demo DIR   (needs CAP_SYS_ADMIN; DIR must be inside a Btrfs subvolume)
fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: sweep_demo DIR");
    let sub = neutra_btrfs::Subvolume::open(std::path::Path::new(&dir))?;
    let start = sub.current_generation()?;
    println!("subvolume {} generation {start}", sub.tree_id());
    println!("press Enter after changing files and running sync");
    std::io::stdin().read_line(&mut String::new())?;
    let changes = sub.changes_since(start)?;
    println!("newest leaf generation {}", changes.generation);
    for (inode, link) in &changes.inodes {
        println!("inode {} {:?} size {} mtime {} parent {:?}", inode.ino, inode.kind, inode.size, inode.mtime, link);
    }
    for dir in &changes.dirs {
        let path = sub.dir_path(*dir)?;
        println!("dir {dir} path {path:?}");
        for child in sub.children(*dir)? {
            println!("    {} -> {} {:?}", child.name, child.ino, child.kind);
        }
    }
    Ok(())
}
