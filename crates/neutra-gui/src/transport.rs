//! GUI transports and their public crate-level entry points.

use crate::app::EventSender;
use crate::Event;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::MountInfo;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub(crate) use file_actions::Action as FileAction;

#[path = "transport/file_actions/mod.rs"]
pub(crate) mod file_actions;
mod local;
pub(crate) mod native_drag;
pub(crate) use local::{spawn_cancellable_local_helper, spawn_local_helper};
#[cfg(target_os = "linux")]
pub(crate) mod indexing_access;

#[path = "transport/adapters.rs"]
mod adapters;
#[path = "transport/domain.rs"]
mod domain;
#[path = "transport/helper_security.rs"]
mod helper_security;
#[path = "transport/state.rs"]
mod state;
#[path = "transport/view.rs"]
mod view;

#[cfg(target_os = "windows")]
pub(crate) use adapters::scan_via_windows_service;
pub(crate) use adapters::spawn_network_watcher;
pub(crate) use domain::{remote_failure_is_offline, scan_has_reachable_lane};
pub(crate) use helper_security::select_helper;
#[cfg(unix)]
pub(crate) use helper_security::validate_elevated_helper;
pub(crate) use state::helper_start_failure;

pub(crate) use view::launch_file_action;

mod network;
