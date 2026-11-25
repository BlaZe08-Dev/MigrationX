//! Core `pg_query` v18 AST analyzer implementation.

use migrationx_contracts::{HazardFinding, StaticFindings, VerificationStatus, SCHEMA_VERSION};
use pg_query::protobuf::{node::Node as NodeEnum, RawStmt};
use tracing::debug;

use crate::error::AnalyzerError;
use crate::rules;

/// Analyzes a PostgreSQL migration SQL string using `pg_query` v18's C parser
/// and returns [`StaticFindings`] populated with detected hazards, lock modes,
/// statement indices, and explicit static analysis limitations.
///
/// # Errors
///
/// Returns [`AnalyzerError::EmptyMigration`] if `migration_sql` is empty or
/// whitespace-only, or [`AnalyzerError::ParseError`] if `pg_query` cannot parse
/// the SQL.
pub fn analyze_migration(migration_sql: &str) -> Result<StaticFindings, AnalyzerError> {
    if migration_sql.trim().is_empty() {
        return Err(AnalyzerError::EmptyMigration);
    }

    let parsed =
        pg_query::parse(migration_sql, pg_query::ParserOptions::default()).map_err(|err| {
            AnalyzerError::ParseError {
                message: err.to_string(),
            }
        })?;

    let mut findings: Vec<HazardFinding> = Vec::new();
    let mut in_tx = false;
    let mut has_lock_timeout = false;
    let mut has_statement_timeout = false;
    let mut has_ddl = false;

    for (stmt_idx, raw_stmt) in parsed.protobuf.stmts.iter().enumerate() {
        let stmt_sql = extract_statement_sql(migration_sql, raw_stmt);
        let Some(node_enum) = raw_stmt.stmt.as_deref().and_then(|n| n.node.as_ref()) else {
            continue;
        };

        debug!(stmt_idx, "analyzing AST statement");

        match node_enum {
            NodeEnum::TransactionStmt(tx) => {
                rules::update_transaction_state(tx, &mut in_tx);
            }
            NodeEnum::VariableSetStmt(vs) => {
                rules::check_variable_set(vs, &mut has_lock_timeout, &mut has_statement_timeout);
            }
            NodeEnum::IndexStmt(idx) => {
                has_ddl = true;
                findings.extend(rules::check_index_stmt(
                    idx,
                    stmt_idx,
                    stmt_sql.as_deref(),
                    in_tx,
                ));
            }
            NodeEnum::DropStmt(drop) => {
                has_ddl = true;
                findings.extend(rules::check_drop_stmt(
                    drop,
                    stmt_idx,
                    stmt_sql.as_deref(),
                    in_tx,
                ));
            }
            NodeEnum::AlterTableStmt(alter) => {
                has_ddl = true;
                findings.extend(rules::check_alter_table_stmt(
                    alter,
                    stmt_idx,
                    stmt_sql.as_deref(),
                ));
            }
            NodeEnum::RenameStmt(rename) => {
                has_ddl = true;
                if let Some(finding) =
                    rules::check_rename_stmt(rename, stmt_idx, stmt_sql.as_deref())
                {
                    findings.push(finding);
                }
            }
            NodeEnum::DoStmt(_) => {
                has_ddl = true;
                findings.push(rules::do_block_finding(stmt_idx, stmt_sql.as_deref()));
            }
            NodeEnum::ExecuteStmt(exec) => {
                has_ddl = true;
                findings.push(rules::execute_stmt_finding(
                    stmt_idx,
                    &exec.name,
                    stmt_sql.as_deref(),
                ));
            }
            NodeEnum::CreateStmt(_)
            | NodeEnum::CreateTableAsStmt(_)
            | NodeEnum::TruncateStmt(_)
            | NodeEnum::ViewStmt(_)
            | NodeEnum::ReindexStmt(_)
            | NodeEnum::ClusterStmt(_) => {
                has_ddl = true;
            }
            _ => {}
        }
    }

    if has_ddl {
        if !has_lock_timeout {
            findings.push(rules::missing_lock_timeout_finding());
        }
        if !has_statement_timeout {
            findings.push(rules::missing_statement_timeout_finding());
        }
    }

    let overall_status = compute_overall_status(&findings);

    Ok(StaticFindings {
        schema_version: SCHEMA_VERSION.to_string(),
        migration_sql: migration_sql.to_string(),
        findings,
        overall_status,
    })
}

/// Extracts the SQL substring for a [`RawStmt`] using its byte offset and length.
fn extract_statement_sql(full_sql: &str, raw_stmt: &RawStmt) -> Option<String> {
    let start = usize::try_from(raw_stmt.stmt_location).ok()?;
    let end = if raw_stmt.stmt_len > 0 {
        let len = usize::try_from(raw_stmt.stmt_len).ok()?;
        start.checked_add(len)?.min(full_sql.len())
    } else {
        full_sql.len()
    };

    full_sql
        .get(start..end)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Computes the aggregate [`VerificationStatus`] without ever conflating
/// absence of static findings with simulation-proven [`VerificationStatus::Passed`].
fn compute_overall_status(findings: &[HazardFinding]) -> VerificationStatus {
    if findings
        .iter()
        .any(|f| f.status == VerificationStatus::StaticallyDetected)
    {
        VerificationStatus::StaticallyDetected
    } else if findings
        .iter()
        .any(|f| f.status == VerificationStatus::UnsupportedScenario)
    {
        VerificationStatus::UnsupportedScenario
    } else {
        VerificationStatus::CouldNotVerify
    }
}
