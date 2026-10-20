# accesskit_unix local backport

This is the crates.io `accesskit_unix` 0.21.1 source with the minimal D-Bus
recoverable-error handling from AccessKit commit
`a7d65458bd8d81ded693c7b8eae451e1e16aaa01` applied to `src/atspi/bus.rs` and
`src/context.rs`. It remains version 0.21.1 to satisfy `accesskit_winit 0.32.x`.

Upstream source: https://github.com/AccessKit/accesskit/tree/f40dfc01a0c0e76de535969f82fb35e19513737d/platforms/unix
License: Apache-2.0 OR MIT; see `LICENSE-APACHE` and `LICENSE-MIT`.

The local compatibility patch uses `org.a11y.Status.IsEnabled` when available
and falls back to `ScreenReaderEnabled` for older launchers. Current AT-SPI
launchers no longer expose the latter property.
