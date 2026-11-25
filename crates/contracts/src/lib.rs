//! # MigrationX Contracts
//!
//! Strongly-typed, versioned schemas shared across all MigrationX crates.
//! This crate defines the data contracts that flow between the static analyzer,
//! simulator, and CLI — ensuring type safety at crate boundaries.

mod schema;

pub use schema::{
    BrokenQuery, HazardFinding, HazardSeverity, LockMode, LockObservation, RuntimeVerdict,
    Snapshot, StaticFindings, TableStats, VerificationStatus, SCHEMA_VERSION,
};
