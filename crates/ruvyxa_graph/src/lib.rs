//! File-system route discovery, validation, rendering-strategy detection, and
//! route manifests.
//!
//! One module per responsibility, and the crate root is the public surface:
//! everything a caller reaches is re-exported here, so `ruvyxa_graph::X` names
//! the same item it always did regardless of which module owns `X`.
//!
//! - `manifest` — the route manifest data model and its serde wire contract,
//!   which two other crates and the deployed handler also read.
//! - `cache_policy` — what a server sends with a document it just rendered.
//! - `discovery` — the `app/` walk, URL segment parsing, and the layout,
//!   template, and sibling-module chains.
//! - `parallel` — parallel-route slots and intercepting routes.
//! - `graph` — the per-run module cache and the reachable-module walk.
//! - `render` — rendering strategy, runtime target, and hydration detection.
//! - `exports` — the route-export text lexing those detectors read through.
//! - `validate` — server/client boundary validation.
//! - `conflicts` — route conflict detection.

mod cache_policy;
mod conflicts;
mod discovery;
mod exports;
mod graph;
mod manifest;
mod parallel;
mod render;
mod validate;

#[cfg(test)]
mod tests;

pub use cache_policy::*;
pub use discovery::*;
pub use exports::*;
pub use manifest::*;
pub use validate::*;
