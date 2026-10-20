//! MCP stdio server exposing Neutrasearch's resident metadata index to agents.
//!
//! This replaces broad filename/path grep/find calls. It does not claim to
//! replace content grep: Neutrasearch intentionally indexes names + metadata.

#[path = "main/adapters.rs"]
mod adapters;
#[path = "main/domain.rs"]
mod domain;
mod policy;
#[path = "main/state.rs"]
mod state;
mod store;
mod tools;
#[path = "main/view.rs"]
mod view;

fn main() -> anyhow::Result<()> {
    adapters::run()
}
