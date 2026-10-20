use anyhow::Result;

pub(crate) fn initialize() -> Result<()> {
    #[cfg(target_os = "linux")]
    unsafe {
        let parent = libc::getppid();
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        anyhow::ensure!(
            parent > 1 && libc::getppid() == parent,
            "scanner parent exited during startup"
        );
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    std::env::set_var("RAYON_NUM_THREADS", "2");
    Ok(())
}
