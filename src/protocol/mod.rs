//! Task-artifact protocol runtime: real folder last-touch + mechanical
//! active⇄deferred reconcile. base stays agnostic — every behavior here is gated
//! on `[protocol] enabled` in base.toml (set by os-config in the global tier and
//! inherited by every scaffolded workspace via the config overlay).

pub mod reconcile;
pub mod touch;

pub use reconcile::{reconcile, ReconcileStats};
