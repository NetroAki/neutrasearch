mod auth;
mod validation;

pub(super) use auth::verify_client;
use validation::validate_client_path;
pub(super) use validation::validate_installed_service_files;
