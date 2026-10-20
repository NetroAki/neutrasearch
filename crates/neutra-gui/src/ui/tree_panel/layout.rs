use egui::Rect;

const ITEMS_W: f32 = 52.0;
const SIZE_W: f32 = 64.0;
const BAR_W: f32 = 34.0;
const NAME_MIN_W: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RowLayout {
    pub(super) indent: f32,
    pub(super) name_right: f32,
    pub(super) bar_right: Option<f32>,
    pub(super) size_right: Option<f32>,
    pub(super) items_right: Option<f32>,
    pub(super) bar_width: f32,
}

/// Keep the name column usable by dropping the lowest-priority metadata first.
/// At deeper levels indentation stops before it consumes the reserved label area.
pub(super) fn row_layout(rect: Rect, depth: usize) -> RowLayout {
    let right = rect.right() - 8.0;
    let mut items_right = Some(right);
    let mut size_right = Some(right - ITEMS_W);
    let mut bar_width = BAR_W;
    let mut bar_right = Some(right - ITEMS_W - SIZE_W);
    let mut name_right = bar_right.unwrap() - bar_width - 8.0;
    let min_name_right = rect.left() + NAME_MIN_W;

    if name_right < min_name_right {
        items_right = None;
        size_right = Some(right);
        bar_right = Some(right - SIZE_W);
        name_right = bar_right.unwrap() - bar_width - 8.0;
    }
    if name_right < min_name_right {
        bar_width = 22.0;
        bar_right = Some(right - SIZE_W);
        name_right = bar_right.unwrap() - bar_width - 8.0;
    }
    if name_right < min_name_right {
        bar_right = None;
        size_right = Some(right);
        name_right = right - SIZE_W - 8.0;
    }
    if name_right < min_name_right {
        size_right = None;
        name_right = right;
    }

    let requested_indent = rect.left() + 10.0 + depth.min(usize::MAX / 14) as f32 * 14.0;
    let indent = requested_indent.min((name_right - 100.0).max(rect.left() + 4.0));
    RowLayout {
        indent,
        name_right,
        bar_right,
        size_right,
        items_right,
        bar_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn bounds(width: f32, depth: usize) -> RowLayout {
        row_layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 26.0)),
            depth,
        )
    }

    #[test]
    fn narrow_rows_yield_metadata_to_keep_a_name_area() {
        let layout = bounds(230.0, 0);
        assert_eq!(layout.items_right, None);
        assert!(layout.size_right.is_some());
        assert!(layout.name_right >= NAME_MIN_W);
    }

    #[test]
    fn very_narrow_rows_keep_name_and_drop_all_metadata() {
        let layout = bounds(150.0, 0);
        assert_eq!(layout.items_right, None);
        assert_eq!(layout.size_right, None);
        assert_eq!(layout.bar_right, None);
        assert!(layout.name_right >= NAME_MIN_W);
    }

    #[test]
    fn deep_indentation_is_capped_before_it_consumes_the_name_area() {
        let shallow = bounds(320.0, 0);
        let deep = bounds(320.0, 100);
        assert!(deep.indent > shallow.indent);
        assert!(deep.indent + 100.0 <= deep.name_right);
        assert!(deep.indent < 320.0);
    }
}
