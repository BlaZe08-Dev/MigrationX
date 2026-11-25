use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Schema version — bump on breaking changes
// ---------------------------------------------------------------------------

/// Current schema version for all contract types.
pub const SCHEMA_VERSION: &str = "0.1.0";

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// A point-in-time capture of the target database's schema and metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Schema version for forward/backward compatibility.
    pub schema_version: String,
    /// ISO-8601 timestamp of when the snapshot was taken.
    pub captured_at: String,
    /// PostgreSQL server version string (e.g. "15.4").
    pub pg_version: String,
    /// Database name.
    pub database_name: String,
    /// Per-table statistics keyed by fully-qualified table name.
    pub tables: HashMap<String, TableStats>,
}

// ---------------------------------------------------------------------------
// TableStats
// ---------------------------------------------------------------------------

/// Statistics for a single table at snapshot time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableStats {
    /// Fully-qualified table name (schema.table).
    pub qualified_name: String,
    /// Estimated row count from `pg_class.reltuples`.
    pub estimated_row_count: i64,
    /// Total table size in bytes (including indexes and TOAST).
    pub total_size_bytes: u64,
    /// Whether the table has any active triggers.
    pub has_triggers: bool,
    /// Whether the table is referenced by foreign keys.
    pub has_foreign_key_refs: bool,
}

// ---------------------------------------------------------------------------
// StaticFindings
// ---------------------------------------------------------------------------

/// Aggregated results from static analysis of a migration script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticFindings {
    pub schema_version: String,
    /// The migration SQL that was analyzed.
    pub migration_sql: String,
    /// Individual hazard findings.
    pub findings: Vec<HazardFinding>,
    /// Overall verification status after static analysis.
    pub overall_status: VerificationStatus,
}

// ---------------------------------------------------------------------------
// HazardFinding
// ---------------------------------------------------------------------------

/// A single migration hazard identified by the static analyzer or simulator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HazardFinding {
    /// Machine-readable hazard code (e.g. "PG001-ACCESS-EXCLUSIVE-LOCK").
    pub code: String,
    /// Human-readable summary of the hazard.
    pub summary: String,
    /// Severity level.
    pub severity: HazardSeverity,
    /// The specific SQL statement that triggers this hazard, if identifiable.
    pub offending_sql: Option<String>,
    /// Verification status of this specific finding.
    pub status: VerificationStatus,
    /// Suggested remediation steps.
    pub remediation: Option<String>,
}

/// Severity classification for hazard findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HazardSeverity {
    /// Will cause downtime or data loss.
    Critical,
    /// High risk of service degradation.
    High,
    /// Moderate risk; may cause brief lock contention.
    Medium,
    /// Low risk; informational or best-practice deviation.
    Low,
}

// ---------------------------------------------------------------------------
// RuntimeVerdict
// ---------------------------------------------------------------------------

/// The result of running a migration in a simulated environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeVerdict {
    pub schema_version: String,
    /// Whether the migration SQL executed without errors.
    pub migration_succeeded: bool,
    /// Wall-clock duration of the migration in milliseconds.
    pub duration_ms: u64,
    /// Lock observations recorded during simulation.
    pub lock_observations: Vec<LockObservation>,
    /// Queries that broke after applying the migration.
    pub broken_queries: Vec<BrokenQuery>,
    /// Overall verification status after simulation.
    pub overall_status: VerificationStatus,
}

// ---------------------------------------------------------------------------
// LockObservation
// ---------------------------------------------------------------------------

/// A lock event observed during migration simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockObservation {
    /// The lock mode acquired (maps to PostgreSQL lock modes).
    pub mode: LockMode,
    /// The relation (table/index) on which the lock was acquired.
    pub relation: String,
    /// Duration the lock was held, in milliseconds.
    pub held_ms: u64,
    /// Whether concurrent queries were blocked by this lock.
    pub blocked_concurrent_queries: bool,
}

/// PostgreSQL lock modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LockMode {
    AccessShare,
    RowShare,
    RowExclusive,
    ShareUpdateExclusive,
    Share,
    ShareRowExclusive,
    Exclusive,
    AccessExclusive,
}

// ---------------------------------------------------------------------------
// BrokenQuery
// ---------------------------------------------------------------------------

/// A query that fails after applying a migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrokenQuery {
    /// The SQL query that failed.
    pub sql: String,
    /// The PostgreSQL error message.
    pub error_message: String,
    /// The SQLSTATE error code (e.g. "42P01" for undefined table).
    pub sqlstate: Option<String>,
}

// ---------------------------------------------------------------------------
// VerificationStatus
// ---------------------------------------------------------------------------

/// Explicit verification status that **never** conflates unverified assumptions
/// with proven safety.
///
/// # Variants
///
/// - `StaticallyDetected` — Hazard identified by static analysis alone.
/// - `ObservedInSimulation` — Hazard confirmed by running in a real PostgreSQL container.
/// - `CouldNotVerify` — The system was unable to verify this claim. **This is NOT safe.**
/// - `Passed` — Verified safe through simulation with no hazards observed.
/// - `UnsupportedScenario` — The scenario is outside the system's analysis capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationStatus {
    /// Hazard was identified by static analysis of the SQL.
    StaticallyDetected,
    /// Hazard was confirmed by observing it in a live simulation.
    ObservedInSimulation,
    /// The system could not verify whether this is safe or hazardous.
    /// **Never treat this as equivalent to `Passed`.**
    CouldNotVerify,
    /// Verified safe — simulation completed with no hazards.
    Passed,
    /// The scenario is outside the system's current analysis capabilities.
    UnsupportedScenario,
}

impl std::fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaticallyDetected => write!(f, "Statically Detected"),
            Self::ObservedInSimulation => write!(f, "Observed In Simulation"),
            Self::CouldNotVerify => write!(f, "Could Not Verify"),
            Self::Passed => write!(f, "Passed"),
            Self::UnsupportedScenario => write!(f, "Unsupported Scenario"),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helper: round-trip JSON serialization
    // -----------------------------------------------------------------------

    fn assert_json_round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string_pretty(value).expect("serialize");
        let deserialized: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(value, &deserialized, "Round-trip failed for {json}");
    }

    // -----------------------------------------------------------------------
    // VerificationStatus
    // -----------------------------------------------------------------------

    #[test]
    fn verification_status_round_trip() {
        let variants = [
            VerificationStatus::StaticallyDetected,
            VerificationStatus::ObservedInSimulation,
            VerificationStatus::CouldNotVerify,
            VerificationStatus::Passed,
            VerificationStatus::UnsupportedScenario,
        ];
        for v in &variants {
            assert_json_round_trip(v);
        }
    }

    #[test]
    fn verification_status_has_exactly_five_variants() {
        // Exhaustive match — if a variant is added, this will fail to compile.
        let all = [
            VerificationStatus::StaticallyDetected,
            VerificationStatus::ObservedInSimulation,
            VerificationStatus::CouldNotVerify,
            VerificationStatus::Passed,
            VerificationStatus::UnsupportedScenario,
        ];
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn verification_status_could_not_verify_is_not_passed() {
        assert_ne!(
            VerificationStatus::CouldNotVerify,
            VerificationStatus::Passed
        );
    }

    #[test]
    fn verification_status_display() {
        assert_eq!(
            VerificationStatus::CouldNotVerify.to_string(),
            "Could Not Verify"
        );
        assert_eq!(VerificationStatus::Passed.to_string(), "Passed");
    }

    #[test]
    fn verification_status_serde_uses_screaming_snake_case() {
        let json =
            serde_json::to_string(&VerificationStatus::StaticallyDetected).expect("serialize");
        assert_eq!(json, "\"STATICALLY_DETECTED\"");

        let json =
            serde_json::to_string(&VerificationStatus::ObservedInSimulation).expect("serialize");
        assert_eq!(json, "\"OBSERVED_IN_SIMULATION\"");

        let json = serde_json::to_string(&VerificationStatus::CouldNotVerify).expect("serialize");
        assert_eq!(json, "\"COULD_NOT_VERIFY\"");
    }

    // -----------------------------------------------------------------------
    // HazardSeverity
    // -----------------------------------------------------------------------

    #[test]
    fn hazard_severity_round_trip() {
        let variants = [
            HazardSeverity::Critical,
            HazardSeverity::High,
            HazardSeverity::Medium,
            HazardSeverity::Low,
        ];
        for v in &variants {
            assert_json_round_trip(v);
        }
    }

    // -----------------------------------------------------------------------
    // LockMode
    // -----------------------------------------------------------------------

    #[test]
    fn lock_mode_round_trip() {
        let variants = [
            LockMode::AccessShare,
            LockMode::RowShare,
            LockMode::RowExclusive,
            LockMode::ShareUpdateExclusive,
            LockMode::Share,
            LockMode::ShareRowExclusive,
            LockMode::Exclusive,
            LockMode::AccessExclusive,
        ];
        for v in &variants {
            assert_json_round_trip(v);
        }
    }

    #[test]
    fn lock_mode_covers_all_pg_modes() {
        // PostgreSQL defines exactly 8 lock modes.
        let all = [
            LockMode::AccessShare,
            LockMode::RowShare,
            LockMode::RowExclusive,
            LockMode::ShareUpdateExclusive,
            LockMode::Share,
            LockMode::ShareRowExclusive,
            LockMode::Exclusive,
            LockMode::AccessExclusive,
        ];
        assert_eq!(all.len(), 8);
    }

    // -----------------------------------------------------------------------
    // TableStats
    // -----------------------------------------------------------------------

    #[test]
    fn table_stats_round_trip() {
        let stats = TableStats {
            qualified_name: "public.users".to_string(),
            estimated_row_count: 1_500_000,
            total_size_bytes: 256 * 1024 * 1024,
            has_triggers: true,
            has_foreign_key_refs: false,
        };
        assert_json_round_trip(&stats);
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_round_trip() {
        let mut tables = HashMap::new();
        tables.insert(
            "public.orders".to_string(),
            TableStats {
                qualified_name: "public.orders".to_string(),
                estimated_row_count: 10_000_000,
                total_size_bytes: 2 * 1024 * 1024 * 1024,
                has_triggers: false,
                has_foreign_key_refs: true,
            },
        );

        let snapshot = Snapshot {
            schema_version: SCHEMA_VERSION.to_string(),
            captured_at: "2026-11-25T08:00:00Z".to_string(),
            pg_version: "15.4".to_string(),
            database_name: "production".to_string(),
            tables,
        };
        assert_json_round_trip(&snapshot);
    }

    #[test]
    fn snapshot_schema_version_matches_constant() {
        let snapshot = Snapshot {
            schema_version: SCHEMA_VERSION.to_string(),
            captured_at: "2026-11-25T08:00:00Z".to_string(),
            pg_version: "16.1".to_string(),
            database_name: "test".to_string(),
            tables: HashMap::new(),
        };
        assert_eq!(snapshot.schema_version, SCHEMA_VERSION);
    }

    // -----------------------------------------------------------------------
    // HazardFinding
    // -----------------------------------------------------------------------

    #[test]
    fn hazard_finding_round_trip() {
        let finding = HazardFinding {
            code: "PG001-ACCESS-EXCLUSIVE-LOCK".to_string(),
            summary: "ALTER TABLE acquires ACCESS EXCLUSIVE lock".to_string(),
            severity: HazardSeverity::Critical,
            offending_sql: Some("ALTER TABLE users ADD COLUMN email TEXT".to_string()),
            status: VerificationStatus::StaticallyDetected,
            remediation: Some("Use ADD COLUMN with a default to avoid rewrite".to_string()),
        };
        assert_json_round_trip(&finding);
    }

    #[test]
    fn hazard_finding_optional_fields_null() {
        let finding = HazardFinding {
            code: "PG002-TABLE-REWRITE".to_string(),
            summary: "Column type change triggers table rewrite".to_string(),
            severity: HazardSeverity::High,
            offending_sql: None,
            status: VerificationStatus::CouldNotVerify,
            remediation: None,
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        assert!(json.contains("\"offending_sql\":null"));
        assert_json_round_trip(&finding);
    }

    // -----------------------------------------------------------------------
    // StaticFindings
    // -----------------------------------------------------------------------

    #[test]
    fn static_findings_round_trip() {
        let findings = StaticFindings {
            schema_version: SCHEMA_VERSION.to_string(),
            migration_sql: "ALTER TABLE users ADD COLUMN age INT;".to_string(),
            findings: vec![HazardFinding {
                code: "PG001-ACCESS-EXCLUSIVE-LOCK".to_string(),
                summary: "ACCESS EXCLUSIVE lock required".to_string(),
                severity: HazardSeverity::Critical,
                offending_sql: Some("ALTER TABLE users ADD COLUMN age INT".to_string()),
                status: VerificationStatus::StaticallyDetected,
                remediation: None,
            }],
            overall_status: VerificationStatus::StaticallyDetected,
        };
        assert_json_round_trip(&findings);
    }

    // -----------------------------------------------------------------------
    // LockObservation
    // -----------------------------------------------------------------------

    #[test]
    fn lock_observation_round_trip() {
        let obs = LockObservation {
            mode: LockMode::AccessExclusive,
            relation: "public.users".to_string(),
            held_ms: 1200,
            blocked_concurrent_queries: true,
        };
        assert_json_round_trip(&obs);
    }

    // -----------------------------------------------------------------------
    // BrokenQuery
    // -----------------------------------------------------------------------

    #[test]
    fn broken_query_round_trip() {
        let bq = BrokenQuery {
            sql: "SELECT email FROM users".to_string(),
            error_message: "column \"email\" does not exist".to_string(),
            sqlstate: Some("42703".to_string()),
        };
        assert_json_round_trip(&bq);
    }

    #[test]
    fn broken_query_without_sqlstate() {
        let bq = BrokenQuery {
            sql: "SELECT * FROM nonexistent".to_string(),
            error_message: "relation \"nonexistent\" does not exist".to_string(),
            sqlstate: None,
        };
        assert_json_round_trip(&bq);
    }

    // -----------------------------------------------------------------------
    // RuntimeVerdict
    // -----------------------------------------------------------------------

    #[test]
    fn runtime_verdict_round_trip() {
        let verdict = RuntimeVerdict {
            schema_version: SCHEMA_VERSION.to_string(),
            migration_succeeded: true,
            duration_ms: 3400,
            lock_observations: vec![LockObservation {
                mode: LockMode::ShareUpdateExclusive,
                relation: "public.orders".to_string(),
                held_ms: 150,
                blocked_concurrent_queries: false,
            }],
            broken_queries: vec![],
            overall_status: VerificationStatus::Passed,
        };
        assert_json_round_trip(&verdict);
    }

    #[test]
    fn runtime_verdict_failed_migration() {
        let verdict = RuntimeVerdict {
            schema_version: SCHEMA_VERSION.to_string(),
            migration_succeeded: false,
            duration_ms: 100,
            lock_observations: vec![],
            broken_queries: vec![BrokenQuery {
                sql: "INSERT INTO audit_log (action) VALUES ('test')".to_string(),
                error_message: "relation \"audit_log\" does not exist".to_string(),
                sqlstate: Some("42P01".to_string()),
            }],
            overall_status: VerificationStatus::ObservedInSimulation,
        };
        assert_json_round_trip(&verdict);
    }

    // -----------------------------------------------------------------------
    // Mock Fixtures
    // -----------------------------------------------------------------------

    #[test]
    fn mock_fixtures_deserialize_cleanly() {
        let snapshot_json = include_str!("../../../tests/fixtures/mocks/sample_snapshot.json");
        let snapshot: Snapshot =
            serde_json::from_str(snapshot_json).expect("deserialize sample_snapshot.json");
        assert_eq!(snapshot.schema_version, SCHEMA_VERSION);
        assert!(!snapshot.tables.is_empty());

        let findings_json =
            include_str!("../../../tests/fixtures/mocks/sample_static_findings.json");
        let findings: StaticFindings =
            serde_json::from_str(findings_json).expect("deserialize sample_static_findings.json");
        assert_eq!(findings.schema_version, SCHEMA_VERSION);
        assert!(!findings.findings.is_empty());

        let verdict_json =
            include_str!("../../../tests/fixtures/mocks/sample_runtime_verdict.json");
        let verdict: RuntimeVerdict =
            serde_json::from_str(verdict_json).expect("deserialize sample_runtime_verdict.json");
        assert_eq!(verdict.schema_version, SCHEMA_VERSION);
        assert!(!verdict.lock_observations.is_empty());
        assert!(!verdict.broken_queries.is_empty());

        let statuses: Vec<VerificationStatus> = findings
            .findings
            .iter()
            .map(|f| f.status)
            .chain(std::iter::once(verdict.overall_status))
            .collect();
        assert!(statuses.contains(&VerificationStatus::StaticallyDetected));
        assert!(statuses.contains(&VerificationStatus::ObservedInSimulation));
        assert!(statuses.contains(&VerificationStatus::CouldNotVerify));
        assert!(statuses.contains(&VerificationStatus::Passed));
        assert!(statuses.contains(&VerificationStatus::UnsupportedScenario));
    }
}
