use crate::*;

/// Result caps: the home view shows the newest slice of the index and typed
/// searches stay at the interactive cap. The status bar reports the full
/// matched count either way ("1,000 of 40,312 results"), so nothing is hidden.
/// Above this many records the launch screen waits for the first search.
pub(crate) const LAUNCH_LISTING_MAX: u64 = 5_000_000;
pub(crate) const HOME_RESULT_CAP: usize = 1_000;
pub(crate) const TYPED_RESULT_CAP: usize = 1_000;

pub(crate) fn embedded_logo() -> (Vec<u8>, u32, u32) {
    let image = image::load_from_memory(include_bytes!("../assets/neutrasearch.png"))
        .expect("embedded Neutrasearch icon must decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    (image.into_raw(), width, height)
}

pub(crate) fn app_icon() -> egui::IconData {
    let (rgba, width, height) = embedded_logo();
    egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Index scans fan out over rayon. Left on every core they saturate the
/// machine for the length of a full-base search, so the GUI keeps a small,
/// low-priority pool: searches take a little longer and nothing else slows.
fn limit_search_threads() {
    // Every thread otherwise grows its own malloc arena and keeps freed
    // pages; two arenas hold the same working set in far less memory.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .start_handler(|_| {
            #[cfg(unix)]
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            }
        })
        .build_global();
}

pub(crate) fn run() -> eframe::Result<()> {
    if let Some(code) = shortcuts::command() {
        std::process::exit(code);
    }
    match terminal::action() {
        terminal::Action::Gui => {}
        terminal::Action::Spotlight => {
            limit_search_threads();
            return quick_search::run();
        }
        terminal::Action::Exit(code) => std::process::exit(code),
    }
    limit_search_threads();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_inner_size([1180.0, 640.0])
            .with_min_inner_size([760.0, 500.0])
            .with_title("Neutrasearch")
            .with_app_id("neutrasearch")
            .with_icon(app_icon()),
        renderer: eframe::Renderer::Glow,
        centered: true,
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "Neutrasearch",
        options,
        Box::new(|cc| Ok(Box::new(app::NeutraApp::new(cc)))),
    )
}
