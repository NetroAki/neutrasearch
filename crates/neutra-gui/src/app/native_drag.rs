use super::NeutraApp;
use crate::transport::native_drag::{self, BeginError, DragEvent};
use std::time::{Duration, Instant};

pub(super) fn update(app: &mut NeutraApp, ctx: &egui::Context, frame: &eframe::Frame) {
    let request = ctx.data_mut(|data| {
        data.remove_temp::<Vec<std::path::PathBuf>>(egui::Id::new("native-file-drag-request"))
    });
    if let Some(paths) = request {
        if app.native_drag.is_none() {
            let result = frame
                .winit_window()
                .ok_or_else(|| BeginError::Failed("Native window is unavailable".into()))
                .and_then(|window| native_drag::begin(window, &paths));
            match result {
                Ok(source) => {
                    egui::DragAndDrop::clear_payload(ctx);
                    ctx.stop_dragging();
                    // The native source owns the button release from here.
                    ctx.input_mut(|input| input.pointer = Default::default());
                    app.native_drag = Some((source, Instant::now()));
                }
                Err(error) => {
                    let message = match error {
                        BeginError::Failed(message) => message,
                        BeginError::Unsupported(reason) => {
                            format!("{}: {}", reason.platform, reason.reason)
                        }
                    };
                    app.file_operations.message = Some((message, true));
                }
            }
        }
    }
    let Some((source, started)) = &mut app.native_drag else {
        return;
    };
    if started.elapsed() > Duration::from_secs(120)
        || ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        source.cancel();
    }
    let mut finished = false;
    for event in source.poll() {
        match event {
            DragEvent::Finished { action } => {
                debug_assert_eq!(action, native_drag::DragAction::Copy);
                finished = true;
            }
            DragEvent::Cancelled => finished = true,
            DragEvent::Offered { actions } => {
                if !actions.contains(&native_drag::DragAction::Copy) {
                    source.cancel();
                }
            }
            DragEvent::Failed(message) => {
                app.file_operations.message = Some((message, true));
                finished = true;
            }
            DragEvent::Started => {}
        }
    }
    if finished {
        app.native_drag = None;
    } else {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
