//! Windows SCM host for the privileged NTFS metadata scanner.
//! The local named pipe accepts only the installed GUI client.

mod domain;
mod permissions;
mod pipe;
mod state;
mod view;

use domain::{validate_installed_service_files, verify_client};
use state::{init_service_logging, service_log};
use state::{wide, PIPE_PATH, STOP_REQUESTED};
use view::report_status;
pub(crate) use view::run;
