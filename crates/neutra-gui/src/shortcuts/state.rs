pub(crate) struct ShortcutSettings {
    pub(crate) open: bool,
    pub(crate) keys: Vec<String>,
    pub(crate) input: String,
    pub(crate) message: Option<Result<(), String>>,
    pub(crate) pending: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        let saved = super::load();
        Self {
            open: false,
            keys: saved
                .as_ref()
                .map(|keys| keys.iter().map(|key| super::keys::label(key)).collect())
                .unwrap_or_default(),
            input: String::new(),
            message: saved.err().map(Err),
            pending: None,
        }
    }
}

impl ShortcutSettings {
    pub(crate) fn start(ctx: &egui::Context) -> Self {
        let settings = Self::default();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let mut settings = settings;
            if settings.message.is_none() {
                settings.apply(ctx);
            }
            settings
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = ctx;
            settings
        }
    }
    pub(crate) fn apply(&mut self, ctx: &egui::Context) {
        let keys = match super::normalize(&self.keys) {
            Ok(keys) => keys,
            Err(error) => {
                self.message = Some(Err(error));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending = Some(rx);
        self.message = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(super::apply(&keys));
            ctx.request_repaint();
        });
    }

    pub(crate) fn poll(&mut self) {
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.message = Some(result);
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.message = Some(Err("Shortcut registration stopped unexpectedly".into()));
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}
