/// What `base --version` prints: the package version plus the commit it was
/// built from.
///
/// `CARGO_PKG_VERSION` alone cannot tell two binaries apart that were built
/// from different branches at the same version. On 2026-09-20 that is exactly
/// what happened: one install silently reverted another's fix and nothing on
/// the machine could say which was running.
///
/// The updater still compares `CARGO_PKG_VERSION` (see `update::run_quiet`).
/// This const changes only what a human is shown, never what is compared.
pub const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("BASE_BUILD_SHA_RESOLVED"),
    ")"
);

pub mod ast_repo;
pub mod graph_analyze;
pub mod graph_extract;
pub mod graph_move;
pub mod graph_query;
pub mod graph_tools;
pub mod llm;
pub mod multimodal;
pub mod apply_ops;
pub mod changelog;
pub mod command;
pub mod config;
pub mod crud;
pub mod dashboard;
pub mod doctor;
pub mod doorbell;
pub mod emit;
pub mod domain;
pub mod extension;
pub mod graph;
pub mod home;
pub mod extract;
pub mod first_run;
pub mod hook;
pub mod install;
pub mod manifest;
pub mod migrate;
pub mod ontology;
pub mod operator;
pub mod plugin;
pub mod protocol;
pub mod relay;
pub mod scaffold;
pub mod scope;
pub mod secret;
pub mod signal;
pub mod standards;
pub mod store;
pub mod supersede;
pub mod update;
