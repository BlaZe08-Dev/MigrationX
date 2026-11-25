use migrationx_contracts::{HazardSeverity, VerificationStatus, SCHEMA_VERSION};
use migrationx_static_analyzer::{analyze_migration, AnalyzerError};

const TIMEOUT_PREAMBLE: &str = "SET lock_timeout = '2s';\nSET statement_timeout = '30s';\n";

#[test]
fn test_static_analyzer_create_index_missing_concurrently_detects_share_lock() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}CREATE INDEX idx_orders_created_at ON public.orders (created_at);"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.schema_version, SCHEMA_VERSION);
    assert_eq!(
        findings.overall_status,
        VerificationStatus::StaticallyDetected
    );
    assert_eq!(findings.findings.len(), 1);

    let f = &findings.findings[0];
    assert_eq!(f.code, "PG003-MISSING-CONCURRENTLY");
    assert_eq!(f.severity, HazardSeverity::High);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("[stmt 2]"));
    assert!(f.summary.contains("SHARE"));
    assert!(f.summary.contains("Limitation:"));
    assert_eq!(
        f.offending_sql.as_deref(),
        Some("CREATE INDEX idx_orders_created_at ON public.orders (created_at)")
    );
}

#[test]
fn test_static_analyzer_create_index_concurrently_outside_tx_no_index_hazard() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}CREATE INDEX CONCURRENTLY idx_orders_created_at ON public.orders (created_at);"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert!(findings.findings.is_empty());
    assert_eq!(findings.overall_status, VerificationStatus::CouldNotVerify);
}

#[test]
fn test_static_analyzer_create_index_concurrently_inside_tx_detects_critical_hazard() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}BEGIN;\nCREATE INDEX CONCURRENTLY idx_orders_created_at ON public.orders (created_at);\nCOMMIT;"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG005-CONCURRENTLY-IN-TRANSACTION");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("[stmt 3]"));
    assert!(f.summary.contains("SHARE UPDATE EXCLUSIVE"));
}

#[test]
fn test_static_analyzer_drop_index_missing_concurrently_detects_access_exclusive() {
    let sql = format!("{TIMEOUT_PREAMBLE}DROP INDEX public.idx_orders_created_at;");
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG004-DROP-INDEX-MISSING-CONCURRENTLY");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("ACCESS EXCLUSIVE"));
}

#[test]
fn test_static_analyzer_add_column_volatile_default_detects_table_rewrite() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}ALTER TABLE public.users ADD COLUMN token UUID NOT NULL DEFAULT gen_random_uuid();"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG001-VOLATILE-DEFAULT-REWRITE");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("ACCESS EXCLUSIVE"));
    assert!(f.summary.contains("gen_random_uuid()"));
    assert!(f.summary.contains("Limitation:"));
}

#[test]
fn test_static_analyzer_add_column_constant_default_no_rewrite_hazard() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}ALTER TABLE public.users ADD COLUMN is_active BOOLEAN NOT NULL DEFAULT true;"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert!(findings.findings.is_empty());
    assert_eq!(findings.overall_status, VerificationStatus::CouldNotVerify);
}

#[test]
fn test_static_analyzer_add_column_unknown_udf_default_returns_could_not_verify() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}ALTER TABLE public.users ADD COLUMN code TEXT DEFAULT custom_hash_fn();"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG001-DEFAULT-VOLATILITY-UNKNOWN");
    assert_eq!(f.status, VerificationStatus::CouldNotVerify);
    assert!(f.summary.contains("pg_proc.provolatile"));
}

#[test]
fn test_static_analyzer_alter_column_type_detects_rewrite_and_access_exclusive() {
    let sql =
        format!("{TIMEOUT_PREAMBLE}ALTER TABLE public.users ALTER COLUMN status TYPE VARCHAR(64);");
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG002-TABLE-REWRITE");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("ACCESS EXCLUSIVE"));
    assert!(f.summary.contains("binary coercibility"));
}

#[test]
fn test_static_analyzer_add_foreign_key_missing_not_valid_detects_share_row_exclusive() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}ALTER TABLE public.orders ADD CONSTRAINT fk_orders_user FOREIGN KEY (user_id) REFERENCES public.users (id);"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG006-FK-MISSING-NOT-VALID");
    assert_eq!(f.severity, HazardSeverity::High);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("SHARE ROW EXCLUSIVE"));
    assert!(f.summary.contains("NOT VALID"));
}

#[test]
fn test_static_analyzer_add_foreign_key_with_not_valid_no_fk_hazard() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}ALTER TABLE public.orders ADD CONSTRAINT fk_orders_user FOREIGN KEY (user_id) REFERENCES public.users (id) NOT VALID;"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert!(findings.findings.is_empty());
    assert_eq!(findings.overall_status, VerificationStatus::CouldNotVerify);
}

#[test]
fn test_static_analyzer_drop_column_detects_n1_breakage() {
    let sql = format!("{TIMEOUT_PREAMBLE}ALTER TABLE public.users DROP COLUMN legacy_email;");
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG007-DROP-COLUMN-N1-BREAKAGE");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("ACCESS EXCLUSIVE"));
    assert!(f.summary.contains("N-1"));
}

#[test]
fn test_static_analyzer_rename_column_detects_n1_breakage() {
    let sql =
        format!("{TIMEOUT_PREAMBLE}ALTER TABLE public.users RENAME COLUMN email TO primary_email;");
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 1);
    let f = &findings.findings[0];
    assert_eq!(f.code, "PG008-RENAME-COLUMN-N1-BREAKAGE");
    assert_eq!(f.severity, HazardSeverity::Critical);
    assert_eq!(f.status, VerificationStatus::StaticallyDetected);
    assert!(f.summary.contains("ACCESS EXCLUSIVE"));
    assert!(f.summary.contains("N-1"));
}

#[test]
fn test_static_analyzer_missing_timeouts_detected_when_ddl_present() {
    let sql = "ALTER TABLE public.users ADD COLUMN bio TEXT;";
    let findings = analyze_migration(sql).expect("analyze");

    let codes: Vec<&str> = findings.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(
        codes,
        vec![
            "PG009-MISSING-LOCK-TIMEOUT",
            "PG010-MISSING-STATEMENT-TIMEOUT"
        ]
    );
    assert_eq!(
        findings.overall_status,
        VerificationStatus::StaticallyDetected
    );
}

#[test]
fn test_static_analyzer_zero_timeouts_still_flagged_as_missing() {
    let sql = "SET lock_timeout = 0;\nSET statement_timeout = '0ms';\nALTER TABLE public.users ADD COLUMN bio TEXT;";
    let findings = analyze_migration(sql).expect("analyze");

    assert_eq!(findings.findings.len(), 2);
}

#[test]
fn test_static_analyzer_do_block_and_execute_return_unsupported_scenario() {
    let sql = format!("{TIMEOUT_PREAMBLE}DO $$ BEGIN PERFORM 1; END $$;\nEXECUTE my_stmt('a');");
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(
        findings.overall_status,
        VerificationStatus::UnsupportedScenario
    );
    assert_eq!(findings.findings.len(), 2);
    assert_eq!(findings.findings[0].code, "PG099-DYNAMIC-DO-BLOCK");
    assert_eq!(
        findings.findings[0].status,
        VerificationStatus::UnsupportedScenario
    );
    assert_eq!(findings.findings[1].code, "PG098-DYNAMIC-EXECUTE");
    assert_eq!(
        findings.findings[1].status,
        VerificationStatus::UnsupportedScenario
    );
}

#[test]
fn test_static_analyzer_empty_and_invalid_sql_return_typed_errors() {
    assert_eq!(
        analyze_migration("   \n\t "),
        Err(AnalyzerError::EmptyMigration)
    );
    let err = analyze_migration("ALTER TABLE ;;;").expect_err("parse error");
    assert!(matches!(err, AnalyzerError::ParseError { .. }));
}

#[test]
fn test_static_analyzer_multi_statement_tracks_statement_index() {
    let sql = format!(
        "{TIMEOUT_PREAMBLE}\
         ALTER TABLE public.users ALTER COLUMN name TYPE TEXT;\n\
         CREATE INDEX idx_users_name ON public.users (name);"
    );
    let findings = analyze_migration(&sql).expect("analyze");

    assert_eq!(findings.findings.len(), 2);
    assert!(findings.findings[0].summary.contains("[stmt 2]"));
    assert!(findings.findings[1].summary.contains("[stmt 3]"));
}

#[test]
fn test_static_analyzer_safe_migration_returns_could_not_verify_never_passed() {
    let sql =
        format!("{TIMEOUT_PREAMBLE}ALTER TABLE public.audit_events ADD COLUMN trace_id UUID;");
    let findings = analyze_migration(&sql).expect("analyze");

    assert!(findings.findings.is_empty());
    assert_eq!(findings.overall_status, VerificationStatus::CouldNotVerify);
}
