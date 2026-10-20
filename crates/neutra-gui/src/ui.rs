use super::*;
use egui::{
    Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Layout, Margin,
    Rect, RichText, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2,
};
use egui_expressive::widgets::SearchField;
use egui_expressive::{ResizableSplit, SplitAxis, Theme};
use neutra_core::{FileKind, FileRecord};
use regex::RegexBuilder;
use std::sync::Arc;
use std::time::Duration;

mod app_frame;
mod category_icons;
mod chrome;
mod dialogs;
pub(crate) mod file_actions;
mod file_interactions;
mod file_shortcuts;
mod hierarchy;
mod icons;
mod pages;
mod results;
mod runtime;
mod shortcut_settings;
use runtime::runtime_state;
mod scopes;
pub(crate) mod script_fonts;
mod sidebar;
mod theme;
mod toolbar;
mod tree_panel;
mod treemap;
pub(super) mod widgets;
mod window;

use dialogs::{about_dialog, diagnostics_dialog};
use sidebar::banner::{banner_color, runtime_banner};
pub(crate) use sidebar::SidebarTab;
use widgets::{
    ancestor_paths, bar_style, extension_color, fixed_strip, fmt_count, format_mtime, format_size,
    ghost_style, mono, parent_path, path_name, primary_button, sans, secondary_button,
    segment_button, shorten, tracked, type_badge, type_color, ACID, ACID_STRONG, ACTIVE, BLACK,
    CANVAS, CAPTION, ERROR, GLOW, HOVER, LINE, LINE_STRONG, MICRO, MUTED, RAISED, SELECTED, SMALL,
    SUBTLE, SURFACE, TEXT, VIOLET, WARN,
};
use widgets::{copy_to_clipboard, paint_search_icon, task_icon};

pub(super) use hierarchy::Hierarchy;
use results::{details_view, grid_view, list_view, perform_file_action, surrender_widget_focus};
use treemap::treemap_view;

mod domain;
pub(crate) use domain::{
    KindFilter, ResultView, RuntimeState, SearchMode, SortMode, BANNER_H, FILTER_H, MENU_H,
    QUERY_H, TOOLBAR_H,
};
mod selection;
use selection::move_result_selection;
mod setup_screens;
use setup_screens::{first_run_view, indexing_view};
mod ready_screen;
use ready_screen::ready_view;
#[cfg(test)]
mod tests;

pub(super) use app_frame::show_app;
