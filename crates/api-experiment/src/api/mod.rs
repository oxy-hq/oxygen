//! The HTTP handlers: thin by construction — extract, call [`crate::experiment`]
//! or [`crate::simulation`], serialize. Mounted by [`crate::workspace_routes`].

pub mod experiment;
pub mod simulation;
