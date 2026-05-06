//! lineage-strategy worker entry point.
//!
//! Boilerplate handled by `lineage_worker_main!`. The macro expands inline so
//! `iii_sdk::register_worker(...)` and `iii.shutdown_async()` remain visibly
//! called from this binary per `cargo expand`.
lineage_worker_macro::lineage_worker_main!(
    "lineage-strategy",
    lineage_strategy::registration::register
);
