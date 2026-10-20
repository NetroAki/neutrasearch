use super::remote_failure_is_offline;
#[path = "adapters/windows_service.rs"]
#[cfg(target_os = "windows")]
mod windows_service;

use crate::app::EventSender;
use crate::Event;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
pub(crate) use windows_service::scan_via_windows_service;

pub(crate) fn spawn_network_watcher(tx: EventSender) {
    std::thread::spawn(move || {
        let provisioner = neutra_remote::Provisioner::from_env();
        let mut ready = std::collections::HashSet::<String>::new();
        let mut last_attempt = BTreeMap::<String, Instant>::new();
        let mut announced_waiting = std::collections::HashSet::<String>::new();
        loop {
            for (host, key) in super::network::discover_network_hosts() {
                if ready.contains(&key)
                    || last_attempt
                        .get(&key)
                        .is_some_and(|attempt| attempt.elapsed() < Duration::from_secs(30))
                {
                    continue;
                }
                last_attempt.insert(key.clone(), Instant::now());
                match provisioner.ensure_installed(&host) {
                    Ok(platform) => {
                        ready.insert(key.clone());
                        announced_waiting.remove(&key);
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!(
                                "helper build {} ready ({:?}/{})",
                                neutra_core::proto::HELPER_BUILD,
                                platform.os,
                                platform.arch
                            ),
                            error: false,
                        });
                    }
                    Err(error) if remote_failure_is_offline(&error) => {
                        if announced_waiting.insert(key.clone()) {
                            let _ = tx.send(Event::Remote {
                                key,
                                status: "offline; waiting to retry when the server is available"
                                    .into(),
                                error: false,
                            });
                        }
                    }
                    Err(error) => {
                        ready.insert(key.clone());
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!("network helper needs attention: {error:#}"),
                            error: true,
                        });
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}
