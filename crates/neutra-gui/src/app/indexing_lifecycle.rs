use super::state::NeutraApp;

pub(super) fn resume(app: &NeutraApp) {
    #[cfg(target_os = "linux")]
    if !app.scanning && !app.building_cache && !app.index_is_empty() {
        crate::transport::indexing_access::resume(app.tx.clone());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
}
