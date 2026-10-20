use crate::app::{search_worker, Event};
use neutra_core::{CompactIndex, MatchFields, Query, SortKey};
use std::sync::{mpsc, Arc};

const WINDOW_SIZE: [f32; 2] = [720.0, 576.0];
const IDLE_HEIGHT: f32 = 84.0;

struct Spotlight {
    index: Option<Arc<CompactIndex>>,
    state: state::SearchState,
    close_requested: bool,
    queue: search_worker::SearchQueue,
    events: mpsc::Receiver<Event>,
    focused: bool,
    select_query_on_focus: bool,
    expanded: Option<bool>,
}

impl Spotlight {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(unix)]
        instance::set_context(cc.egui_ctx.clone());
        crate::ui::widgets::configure(&cc.egui_ctx);
        let (tx, events) = mpsc::channel();
        let tx = crate::app::EventSender::new(tx, cc.egui_ctx.clone());
        let result = CompactIndex::open_fast(&crate::compact_cache_path());
        let error = result
            .as_ref()
            .err()
            .map(|error| format!("Cannot open the index: {error}"));
        let index = result.ok().map(Arc::new);
        let queue = search_worker::spawn_immediate(tx, index.clone(), crate::compact_cache_path());
        let app = Self {
            index,
            state: state::SearchState {
                error,
                ..Default::default()
            },
            close_requested: false,
            queue,
            events,
            focused: false,
            select_query_on_focus: false,
            expanded: None,
        };
        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        app
    }

    fn search(&mut self) {
        let id = self.state.begin();
        self.queue.invalidate(id);
        if !self.state.pending {
            return;
        }
        let path = crate::compact_cache_path();
        let generation = CompactIndex::generation_on_disk(&path).ok();
        if self.index.as_ref().map(|index| index.generation()) != generation || self.index.is_none()
        {
            match CompactIndex::open_fast(&path) {
                Ok(index) => self.index = Some(Arc::new(index)),
                Err(error) => {
                    self.state
                        .complete(id, Err(format!("Cannot open the index: {error}")));
                    return;
                }
            }
        }
        let Some(index) = self.index.as_ref() else {
            return;
        };
        let mut query = Query::parse(&self.state.query);
        query.match_fields = MatchFields::Name;
        query.limit = 8;
        query.sort = SortKey::Relevance;
        if let Err(error) = self.queue.send(search_worker::SearchJob {
            id,
            query,
            index: index.clone(),
            index_path: crate::compact_cache_path(),
            offset: 0,
            after: None,
        }) {
            self.state.complete(id, Err(error));
        }
    }

    fn open(&mut self) {
        if let Some(hit) = self.state.selected_hit() {
            match crate::launch_file_action(crate::FileAction::Open(
                hit.record.path.as_ref().into(),
            )) {
                Ok(()) => self.close_requested = true,
                Err(error) => self.state.error = Some(error.to_string()),
            }
        }
    }
}

impl eframe::App for Spotlight {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(unix)]
        if instance::take_focus_request() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            self.focused = false;
            self.select_query_on_focus = true;
        }
        while let Ok(event) = self.events.try_recv() {
            if let Event::SearchDone { id, result } = event {
                self.state.complete(id, result);
            }
        }
        self.show(ui);
        if self.close_requested || ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub(crate) fn run() -> eframe::Result<()> {
    #[cfg(windows)]
    let _instance = match windows_instance::claim_or_focus() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => return Err(eframe::Error::AppCreation(Box::new(error))),
    };
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
            .with_inner_size([WINDOW_SIZE[0], IDLE_HEIGHT])
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
mod instance;
#[cfg(windows)]
mod windows_instance;

mod rows;
mod state;
#[cfg(test)]
mod tests;
mod view;
