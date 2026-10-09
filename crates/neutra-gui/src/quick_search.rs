use crate::app::{search_worker, Event};
use neutra_core::{CompactIndex, MatchFields, Query, SearchHit, SortKey};
use std::sync::{mpsc, Arc};

struct Spotlight {
    index: Option<Arc<CompactIndex>>,
    query: String,
    hits: Vec<SearchHit>,
    error: Option<String>,
    selected: usize,
    sequence: u64,
    queue: search_worker::SearchQueue,
    events: mpsc::Receiver<Event>,
    focused: bool,
}

impl Spotlight {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(unix)]
        instance::set_context(cc.egui_ctx.clone());
        crate::ui::widgets::configure(&cc.egui_ctx);
        let (tx, events) = mpsc::channel();
        let result = CompactIndex::open_fast(&crate::compact_cache_path());
        let error = result
            .as_ref()
            .err()
            .map(|error| format!("Cannot open the index: {error}"));
        let mut app = Self {
            index: result.ok().map(Arc::new),
            query: String::new(),
            hits: Vec::new(),
            error,
            selected: 0,
            sequence: 0,
            queue: search_worker::spawn(tx, cc.egui_ctx.clone()),
            events,
            focused: false,
        };
        app.search();
        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        app
    }

    fn search(&mut self) {
        let Some(index) = self.index.as_ref() else {
            return;
        };
        self.sequence += 1;
        let mut query = Query::parse(&self.query);
        query.match_fields = MatchFields::Name;
        query.limit = 8;
        query.sort = if self.query.trim().is_empty() {
            SortKey::MtimeDesc
        } else {
            SortKey::Relevance
        };
        if let Err(error) = self.queue.send(search_worker::SearchJob {
            id: self.sequence,
            query,
            index: index.clone(),
            index_path: crate::compact_cache_path(),
            offset: 0,
            after: None,
        }) {
            self.error = Some(error.to_string());
        }
    }

    fn open(&mut self) {
        if let Some(hit) = self.hits.get(self.selected) {
            match crate::launch_file_action(crate::FileAction::Open(
                hit.record.path.as_ref().into(),
            )) {
                Ok(()) => self.error = Some(String::new()),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }
}

impl eframe::App for Spotlight {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, crate::ui::widgets::CANVAS);
        #[cfg(unix)]
        if instance::take_focus_request() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        while let Ok(event) = self.events.try_recv() {
            if let Event::SearchDone { id, result } = event {
                if id != self.sequence {
                    continue;
                }
                match result {
                    Ok((hits, _)) => {
                        self.hits = hits;
                        self.selected = 0;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.add_space(8.0);
        let field = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::TextEdit::singleline(&mut self.query).hint_text("Search file names…"),
        );
        if !self.focused {
            field.request_focus();
            if let Some(size) = ui.input(|input| input.viewport().monitor_size) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        (size.x - 680.0).max(0.0) / 2.0,
                        size.y * 0.2,
                    )));
            }
            self.focused = true;
        }
        if field.changed() {
            self.search();
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) && !self.hits.is_empty() {
            self.selected = (self.selected + 1).min(self.hits.len() - 1);
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            self.selected = self.selected.saturating_sub(1);
        }
        if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.open();
        }
        ui.add_space(6.0);
        let mut clicked = None;
        for (index, hit) in self.hits.iter().enumerate() {
            crate::ui::script_fonts::ensure(ui.ctx(), &hit.record.path);
            if ui
                .selectable_label(
                    index == self.selected,
                    format!("{}\n{}", hit.record.name(), hit.record.path),
                )
                .clicked()
            {
                clicked = Some(index);
            }
        }
        if let Some(index) = clicked {
            self.selected = index;
            self.open();
        }
        if let Some(error) = &self.error {
            if error.is_empty() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.label(egui::RichText::new(error).color(egui::Color32::from_rgb(248, 113, 113)));
            }
        }
        ui.add_space(6.0);
        ui.label("Enter opens · Up/Down selects · Esc closes");
    }
}

pub(crate) fn run() -> eframe::Result<()> {
    #[cfg(unix)]
    // Single-instance: a second `--spotlight` focuses the first window and
    // exits. When the user-runtime socket cannot be established (no
    // XDG_RUNTIME_DIR, unwritable dir) the window still opens; only the
    // focus handoff is degraded. This binding stays alive for the window
    // lifetime via `_instance`.
    #[cfg(unix)]
    let _instance = match instance::claim_or_focus() {
        Ok(instance::Claim::Primary(guard)) => Some(guard),
        Ok(instance::Claim::FocusedExisting) => return Ok(()),
        Err(error) => {
            eprintln!("neutrasearch: spotlight single-instance unavailable ({error}); opening without focus handoff");
            None
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Neutrasearch Quick Search")
            .with_app_id("neutrasearch-quick-search")
            .with_inner_size([680.0, 410.0])
            .with_decorations(false)
            .with_resizable(false)
            .with_active(true),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Neutrasearch Quick Search",
        options,
        Box::new(|cc| Ok(Box::new(Spotlight::new(cc)))),
    )
}

#[cfg(unix)]
mod instance {
    use std::io::{self, Read, Write};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    const MESSAGE: &[u8] = b"focus\n";

    pub(super) enum Claim {
        Primary(Guard),
        FocusedExisting,
    }

    pub(super) struct Guard {
        path: PathBuf,
        inode: u64,
        device: u64,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
                use std::os::unix::fs::MetadataExt;
                if metadata.ino() == self.inode && metadata.dev() == self.device {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
    }

    pub(super) fn claim_or_focus() -> io::Result<Claim> {
        let path = socket_path()?;
        match UnixListener::bind(&path) {
            Ok(listener) => {
                listener.set_nonblocking(true)?;
                let guard = guard_for(&path)?;
                let _ = std::thread::Builder::new()
                    .name("spotlight-activation".into())
                    .spawn(move || accept_focus(listener))?;
                Ok(Claim::Primary(guard))
            }
            Err(bind_error) if bind_error.kind() == io::ErrorKind::AddrInUse => {
                match focus_existing(&path) {
                    Ok(()) => Ok(Claim::FocusedExisting),
                    Err(_) => {
                        remove_stale_socket(&path)?;
                        let listener = UnixListener::bind(&path).map_err(|_| bind_error)?;
                        listener.set_nonblocking(true)?;
                        let guard = guard_for(&path)?;
                        let _ = std::thread::Builder::new()
                            .name("spotlight-activation".into())
                            .spawn(move || accept_focus(listener))?;
                        Ok(Claim::Primary(guard))
                    }
                }
            }
            Err(error) => Err(error),
        }
    }

    fn guard_for(path: &PathBuf) -> io::Result<Guard> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Guard {
            path: path.clone(),
            inode: metadata.ino(),
            device: metadata.dev(),
        })
    }

    fn socket_path() -> io::Result<PathBuf> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unavailable")
            })?;
        Ok(runtime.join("neutrasearch-quick-search.sock"))
    }

    fn focus_existing(path: &PathBuf) -> io::Result<()> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        stream.write_all(MESSAGE)
    }

    fn remove_stale_socket(path: &PathBuf) -> io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "instance path is not a socket",
            ));
        }
        use std::os::unix::fs::MetadataExt;
        let expected_uid = unsafe { libc::geteuid() };
        if metadata.uid() != expected_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "instance socket belongs to another user",
            ));
        }
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "spotlight instance is active",
                ))
            }
            Err(error)
                if error.kind() == io::ErrorKind::ConnectionRefused
                    || error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let current = std::fs::symlink_metadata(path)?;
        if current.ino() != metadata.ino() || current.dev() != metadata.dev() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "instance socket changed during stale-socket check",
            ));
        }
        std::fs::remove_file(path)
    }

    fn accept_focus(listener: UnixListener) {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                    let mut message = [0; 6];
                    if stream.read_exact(&mut message).is_ok() && message == MESSAGE {
                        FOCUS_PENDING.store(true, Ordering::Release);
                        if let Some(ctx) = CONTEXT.get() {
                            ctx.request_repaint();
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return,
            }
        }
    }

    static FOCUS_PENDING: AtomicBool = AtomicBool::new(false);
    static CONTEXT: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

    pub(super) fn set_context(ctx: egui::Context) {
        let _ = CONTEXT.set(ctx);
    }

    pub(super) fn take_focus_request() -> bool {
        FOCUS_PENDING.swap(false, Ordering::AcqRel)
    }
}
