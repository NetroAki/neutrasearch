pub(crate) fn remote_failure_is_offline(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "connection timed out",
        "connection refused",
        "no route to host",
        "network is unreachable",
        "could not resolve hostname",
        "name or service not known",
        "operation timed out",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}
