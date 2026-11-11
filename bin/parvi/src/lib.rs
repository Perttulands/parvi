//! The node's list of areas, which `main` serves and tests check.

use parvi_node_kit::Listing;

/// The node's areas, in the order they are built. Adding an area adds one
/// line here and one in Cargo.toml.
pub const AREAS: &[Listing] = &[
    parvi_agents::LISTING,
    parvi_terminals::LISTING,
    parvi_scheduled::LISTING,
    parvi_host::LISTING,
    parvi_work::LISTING,
    parvi_brain::LISTING,
    parvi_files::LISTING,
];

/// The binary's own first words, which no area's command noun may take.
pub const VERBS: &[&str] = &["serve", "mcp", "hook", "kinds", "help"];
