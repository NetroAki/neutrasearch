mod domain;
mod view;

#[cfg(test)]
mod tests;

pub use view::{action, Action};

use crate::{
    default_system_roots, record_in_roots, scan_has_reachable_lane, selected_scan_mounts,
    spawn_local_helper, Event,
};
