#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod bootstrap;
mod mount_scope;
mod paths;
mod quick_search;
mod settings;
mod shortcuts;
mod startup_access;
mod terminal;
mod transport;
mod ui;
use std::path::PathBuf;
#[cfg(test)]
mod security_tests;

pub(crate) use app::{Event, GuiSettings, LaneState, NeutraApp};
use bootstrap::{embedded_logo, HOME_RESULT_CAP, LAUNCH_LISTING_MAX, TYPED_RESULT_CAP};
use mount_scope::{
    default_system_roots, normalize_selected_root, portable_path_in_root, record_in_roots,
    same_root, scope_within_selected_roots, selected_scan_mounts,
};
use paths::{compact_cache_path, env_flag, legacy_cache_path};
use settings::{gui_settings_path, load_gui_settings, save_gui_settings};
use startup_access::request_elevated_restart;
pub(crate) use transport::{
    launch_file_action, scan_has_reachable_lane, spawn_local_helper, FileAction,
};

fn main() -> eframe::Result<()> {
    bootstrap::run()
}
