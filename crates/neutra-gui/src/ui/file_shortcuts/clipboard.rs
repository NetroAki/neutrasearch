pub(super) fn consume(ui: &mut egui::Ui, key: egui::Key) -> bool {
    ui.input_mut(|input| {
        let native = consume_event(&mut input.events, key);
        input.consume_key(egui::Modifiers::COMMAND, key) || native
    })
}

fn consume_event(events: &mut Vec<egui::Event>, key: egui::Key) -> bool {
    let before = events.len();
    events.retain(|event| {
        !matches!(
            (key, event),
            (egui::Key::C, egui::Event::Copy)
                | (egui::Key::X, egui::Event::Cut)
                | (egui::Key::V, egui::Event::Paste(_))
        )
    });
    events.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_clipboard_commands_are_consumed_once_without_eating_text() {
        let mut events = vec![
            egui::Event::Copy,
            egui::Event::Cut,
            egui::Event::Paste("file:///tmp/probe".into()),
            egui::Event::Text("a".into()),
        ];
        for key in [egui::Key::C, egui::Key::X, egui::Key::V] {
            assert!(consume_event(&mut events, key));
            assert!(!consume_event(&mut events, key));
        }
        assert_eq!(events, vec![egui::Event::Text("a".into())]);
    }
}
