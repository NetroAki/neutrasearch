use super::*;

impl eframe::App for NeutraApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(target_os = "linux")]
        if self.scanning || self.building_cache {
            self.scan_cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            crate::transport::indexing_access::resume_after_exit();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Event pump first so the frame paints fresh state; queries and scans
        // read the same fields the UI draws.
        super::super::events::process_events(self);
        ui::show_app(self, ui);
        super::super::native_drag::update(self, ui.ctx(), frame);
    }
}
