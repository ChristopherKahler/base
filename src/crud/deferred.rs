//! Deferred state (spec Part C): what `base <type> deferred` lists, the keys it hands out, and
//! revival through `base handoff show` and `base fork show`.

use std::path::Path;

use anyhow::Result;

use crate::config::{BaseConfig, DeferKind};

/// `base <type> deferred`: every deferred record of `kind`, in every tier.
pub fn list(gbl_root: Option<&Path>, cwd: &Path, config: &BaseConfig, kind: DeferKind) -> Result<()> {
    // Law 11 commit 1: the surface, no behaviour.
    let _ = (gbl_root, cwd, config, kind);
    Ok(())
}
