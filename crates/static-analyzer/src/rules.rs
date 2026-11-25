//! AST hazard detection rules for PostgreSQL migration statements.
//!
//! Each rule inspects a specific `pg_query::protobuf::node::Node` variant and
//! produces [`HazardFinding`] items annotated with the statement index,
//! PostgreSQL [`LockMode`] impact, [`VerificationStatus`], and explicit static
//! analysis limitations.

use migrationx_contracts::{HazardFinding, HazardSeverity, LockMode, VerificationStatus};
use pg_query::protobuf::{
    node::Node as NodeEnum, AlterTableCmd, AlterTableStmt, AlterTableType, ColumnDef, ConstrType,
    Constraint, DropStmt, IndexStmt, Node, ObjectType, RangeVar, RenameStmt, TransactionStmt,
    TransactionStmtKind, VariableSetKind, VariableSetStmt,
};

/// Known volatile PostgreSQL built-in functions that force a table rewrite
/// when used in `ALTER TABLE ... ADD COLUMN ... DEFAULT <expr>`.
const KNOWN_VOLATILE_FUNCTIONS: &[&str] = &[
    "random",
    "setseed",
    "clock_timestamp",
    "timeofday",
    "gen_random_uuid",
    "uuid_generate_v1",
    "uuid_generate_v1mc",
    "uuid_generate_v4",
    "uuid_generate_v5",
    "nextval",
    "currval",
    "setval",
    "lastval",
    "txid_current",
    "txid_current_if_assigned",
    "pg_current_xact_id",
    "pg_current_xact_id_if_assigned",
];

/// Known non-volatile (immutable/stable) PostgreSQL built-in functions that
/// do NOT trigger a table rewrite on PostgreSQL 11+ when used as a column default.
const KNOWN_NON_VOLATILE_FUNCTIONS: &[&str] = &[
    "now",
    "current_timestamp",
    "transaction_timestamp",
    "statement_timestamp",
    "clock_date",
    "lower",
    "upper",
    "trim",
    "btrim",
    "ltrim",
    "rtrim",
    "coalesce",
    "nullif",
    "concat",
    "concat_ws",
    "length",
    "char_length",
    "character_length",
    "abs",
    "ceil",
    "ceiling",
    "floor",
    "round",
    "jsonb_build_object",
    "jsonb_build_array",
    "json_build_object",
    "json_build_array",
    "to_jsonb",
    "to_json",
];

/// Classification of a `DEFAULT` expression on `ADD COLUMN`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultVolatility {
    /// Proven constant literal or known immutable/stable expression (PG 11+ fast default).
    ConstantOrStable,
    /// Known volatile expression that forces a full table rewrite.
    Volatile(String),
    /// Expression whose volatility cannot be determined without `pg_proc` catalog metadata.
    Unknown(String),
}

/// Formats a [`RangeVar`] into `schema.table` or `table`.
pub(crate) fn format_range_var(rv: Option<&RangeVar>) -> String {
    match rv {
        Some(r) if !r.schemaname.is_empty() => format!("{}.{}", r.schemaname, r.relname),
        Some(r) if !r.relname.is_empty() => r.relname.clone(),
        _ => "<unknown>".to_string(),
    }
}

/// Formats a PostgreSQL [`LockMode`] into its canonical SQL lock name.
pub(crate) fn lock_mode_label(mode: LockMode) -> &'static str {
    match mode {
        LockMode::AccessShare => "ACCESS SHARE",
        LockMode::RowShare => "ROW SHARE",
        LockMode::RowExclusive => "ROW EXCLUSIVE",
        LockMode::ShareUpdateExclusive => "SHARE UPDATE EXCLUSIVE",
        LockMode::Share => "SHARE",
        LockMode::ShareRowExclusive => "SHARE ROW EXCLUSIVE",
        LockMode::Exclusive => "EXCLUSIVE",
        LockMode::AccessExclusive => "ACCESS EXCLUSIVE",
    }
}

/// Formats a list of `pg_query` `NodeEnum::String` items as a dotted identifier.
fn format_dotted_names(nodes: &[Node]) -> String {
    let parts: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        "<unknown>".to_string()
    } else {
        parts.join(".")
    }
}

/// Updates explicit transaction boundary tracking for [`TransactionStmt`].
pub(crate) fn update_transaction_state(stmt: &TransactionStmt, in_tx: &mut bool) {
    if let Ok(kind) = TransactionStmtKind::try_from(stmt.kind) {
        match kind {
            TransactionStmtKind::TransStmtBegin | TransactionStmtKind::TransStmtStart => {
                *in_tx = true;
            }
            TransactionStmtKind::TransStmtCommit
            | TransactionStmtKind::TransStmtRollback
            | TransactionStmtKind::TransStmtPrepare
            | TransactionStmtKind::TransStmtCommitPrepared
            | TransactionStmtKind::TransStmtRollbackPrepared => {
                *in_tx = false;
            }
            TransactionStmtKind::Undefined
            | TransactionStmtKind::TransStmtSavepoint
            | TransactionStmtKind::TransStmtRelease
            | TransactionStmtKind::TransStmtRollbackTo => {}
        }
    }
}

/// Checks a [`VariableSetStmt`] for `SET lock_timeout` or `SET statement_timeout`.
pub(crate) fn check_variable_set(
    stmt: &VariableSetStmt,
    has_lock_timeout: &mut bool,
    has_statement_timeout: &mut bool,
) {
    if let Ok(kind) = VariableSetKind::try_from(stmt.kind) {
        if kind != VariableSetKind::VarSetValue {
            return;
        }
    }
    let name = stmt.name.to_ascii_lowercase();
    if name == "lock_timeout" && !is_zero_timeout(&stmt.args) {
        *has_lock_timeout = true;
    } else if name == "statement_timeout" && !is_zero_timeout(&stmt.args) {
        *has_statement_timeout = true;
    }
}

/// Returns `true` if the `SET` argument explicitly disables a timeout (`0`, `'0'`, `'0ms'`, etc.).
fn is_zero_timeout(args: &[Node]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };
    let Some(NodeEnum::AConst(aconst)) = first.node.as_ref() else {
        return false;
    };
    match aconst.val.as_ref() {
        Some(pg_query::protobuf::a_const::Val::Ival(iv)) => iv.ival == 0,
        Some(pg_query::protobuf::a_const::Val::Sval(sv)) => {
            let trimmed = sv.sval.trim().to_ascii_lowercase();
            trimmed == "0" || trimmed == "0ms" || trimmed == "0s" || trimmed == "0min"
        }
        _ => false,
    }
}

/// Checks `CREATE INDEX` for missing `CONCURRENTLY` or `CONCURRENTLY` inside a transaction.
pub(crate) fn check_index_stmt(
    stmt: &IndexStmt,
    stmt_idx: usize,
    offending_sql: Option<&str>,
    in_tx: bool,
) -> Vec<HazardFinding> {
    let mut findings = Vec::new();
    let table = format_range_var(stmt.relation.as_ref());

    if !stmt.concurrent {
        let lock = LockMode::Share;
        findings.push(HazardFinding {
            code: "PG003-MISSING-CONCURRENTLY".to_string(),
            summary: format!(
                "[stmt {stmt_idx}] CREATE INDEX on {table} without CONCURRENTLY acquires {} lock, \
                 blocking concurrent writes (INSERT/UPDATE/DELETE). \
                 Limitation: static analysis cannot estimate table size or build duration without catalog stats.",
                lock_mode_label(lock)
            ),
            severity: HazardSeverity::High,
            offending_sql: offending_sql.map(str::to_string),
            status: VerificationStatus::StaticallyDetected,
            remediation: Some(
                "Use CREATE INDEX CONCURRENTLY outside of a transaction block (acquires SHARE UPDATE EXCLUSIVE)."
                    .to_string(),
            ),
        });
    } else if in_tx {
        let lock = LockMode::ShareUpdateExclusive;
        findings.push(HazardFinding {
            code: "PG005-CONCURRENTLY-IN-TRANSACTION".to_string(),
            summary: format!(
                "[stmt {stmt_idx}] CREATE INDEX CONCURRENTLY on {table} ({} lock) cannot execute \
                 inside an explicit transaction block and will fail at runtime. \
                 Limitation: implicit transaction wrappers added by migration runners are not visible in SQL alone.",
                lock_mode_label(lock)
            ),
            severity: HazardSeverity::Critical,
            offending_sql: offending_sql.map(str::to_string),
            status: VerificationStatus::StaticallyDetected,
            remediation: Some(
                "Move CREATE INDEX CONCURRENTLY outside BEGIN/COMMIT or disable transaction wrapping in the migration runner."
                    .to_string(),
            ),
        });
    }

    findings
}

/// Checks `DROP INDEX` for missing `CONCURRENTLY` or `CONCURRENTLY` inside a transaction.
pub(crate) fn check_drop_stmt(
    stmt: &DropStmt,
    stmt_idx: usize,
    offending_sql: Option<&str>,
    in_tx: bool,
) -> Vec<HazardFinding> {
    let mut findings = Vec::new();
    let Ok(obj_type) = ObjectType::try_from(stmt.remove_type) else {
        return findings;
    };

    if obj_type == ObjectType::ObjectIndex {
        let targets: Vec<String> = stmt
            .objects
            .iter()
            .filter_map(|o| match o.node.as_ref() {
                Some(NodeEnum::List(list)) => Some(format_dotted_names(&list.items)),
                _ => None,
            })
            .collect();
        let target_desc = if targets.is_empty() {
            "<unknown>".to_string()
        } else {
            targets.join(", ")
        };

        if !stmt.concurrent {
            let lock = LockMode::AccessExclusive;
            findings.push(HazardFinding {
                code: "PG004-DROP-INDEX-MISSING-CONCURRENTLY".to_string(),
                summary: format!(
                    "[stmt {stmt_idx}] DROP INDEX on {target_desc} without CONCURRENTLY acquires {} lock \
                     on the parent table, blocking all concurrent reads and writes. \
                     Limitation: parent table contention depends on active traffic.",
                    lock_mode_label(lock)
                ),
                severity: HazardSeverity::Critical,
                offending_sql: offending_sql.map(str::to_string),
                status: VerificationStatus::StaticallyDetected,
                remediation: Some(
                    "Use DROP INDEX CONCURRENTLY outside of a transaction block.".to_string(),
                ),
            });
        } else if in_tx {
            let lock = LockMode::ShareUpdateExclusive;
            findings.push(HazardFinding {
                code: "PG005-CONCURRENTLY-IN-TRANSACTION".to_string(),
                summary: format!(
                    "[stmt {stmt_idx}] DROP INDEX CONCURRENTLY on {target_desc} ({} lock) cannot execute \
                     inside an explicit transaction block and will fail at runtime.",
                    lock_mode_label(lock)
                ),
                severity: HazardSeverity::Critical,
                offending_sql: offending_sql.map(str::to_string),
                status: VerificationStatus::StaticallyDetected,
                remediation: Some(
                    "Move DROP INDEX CONCURRENTLY outside BEGIN/COMMIT.".to_string(),
                ),
            });
        }
    }

    findings
}

/// Checks all subcommands of an `ALTER TABLE` statement.
pub(crate) fn check_alter_table_stmt(
    stmt: &AlterTableStmt,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> Vec<HazardFinding> {
    let mut findings = Vec::new();
    let table = format_range_var(stmt.relation.as_ref());

    for cmd_node in &stmt.cmds {
        let Some(NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() else {
            continue;
        };
        let Ok(subtype) = AlterTableType::try_from(cmd.subtype) else {
            continue;
        };

        match subtype {
            AlterTableType::AtAddColumn => {
                if let Some(finding) = check_add_column(cmd, &table, stmt_idx, offending_sql) {
                    findings.push(finding);
                }
            }
            AlterTableType::AtAlterColumnType => {
                findings.push(alter_column_type_finding(
                    &table,
                    &cmd.name,
                    stmt_idx,
                    offending_sql,
                ));
            }
            AlterTableType::AtAddConstraint => {
                if let Some(finding) = check_add_constraint(cmd, &table, stmt_idx, offending_sql) {
                    findings.push(finding);
                }
            }
            AlterTableType::AtDropColumn => {
                findings.push(drop_column_finding(
                    &table,
                    &cmd.name,
                    stmt_idx,
                    offending_sql,
                ));
            }
            _ => {}
        }
    }

    findings
}

/// Checks `ALTER TABLE ... ADD COLUMN ... DEFAULT <expr>` for volatile defaults.
fn check_add_column(
    cmd: &AlterTableCmd,
    table: &str,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> Option<HazardFinding> {
    let def_node = cmd.def.as_deref()?;
    let Some(NodeEnum::ColumnDef(col_def)) = def_node.node.as_ref() else {
        return None;
    };

    let default_expr = extract_column_default_expr(col_def)?;
    let col_name = &col_def.colname;
    let lock = LockMode::AccessExclusive;

    match classify_default_volatility(default_expr) {
        DefaultVolatility::ConstantOrStable => None,
        DefaultVolatility::Volatile(expr_desc) => Some(HazardFinding {
            code: "PG001-VOLATILE-DEFAULT-REWRITE".to_string(),
            summary: format!(
                "[stmt {stmt_idx}] ADD COLUMN {table}.{col_name} with volatile DEFAULT ({expr_desc}) \
                 triggers a full table rewrite while holding {} lock (all PostgreSQL versions). \
                 Limitation: rewrite duration depends on table size, which is not available statically.",
                lock_mode_label(lock)
            ),
            severity: HazardSeverity::Critical,
            offending_sql: offending_sql.map(str::to_string),
            status: VerificationStatus::StaticallyDetected,
            remediation: Some(
                "Add the column without a volatile DEFAULT (or nullable), backfill in batches, \
                 then set the default for new rows."
                    .to_string(),
            ),
        }),
        DefaultVolatility::Unknown(expr_desc) => Some(HazardFinding {
            code: "PG001-DEFAULT-VOLATILITY-UNKNOWN".to_string(),
            summary: format!(
                "[stmt {stmt_idx}] ADD COLUMN {table}.{col_name} has DEFAULT ({expr_desc}) under {} lock \
                 whose volatility cannot be determined statically; if volatile (or PG < 11), it triggers a full table rewrite. \
                 Limitation: function volatility (pg_proc.provolatile) requires catalog inspection.",
                lock_mode_label(lock)
            ),
            severity: HazardSeverity::High,
            offending_sql: offending_sql.map(str::to_string),
            status: VerificationStatus::CouldNotVerify,
            remediation: Some(
                "Verify the function is IMMUTABLE or STABLE in pg_proc, or add the column nullable and backfill in batches."
                    .to_string(),
            ),
        }),
    }
}

/// Extracts the default expression node from a [`ColumnDef`] (either `raw_default` or `ConstrDefault`).
fn extract_column_default_expr(col_def: &ColumnDef) -> Option<&Node> {
    if let Some(raw) = col_def.raw_default.as_deref() {
        return Some(raw);
    }
    for c_node in &col_def.constraints {
        if let Some(NodeEnum::Constraint(c)) = c_node.node.as_ref() {
            if ConstrType::try_from(c.contype) == Ok(ConstrType::ConstrDefault) {
                if let Some(raw) = c.raw_expr.as_deref() {
                    return Some(raw);
                }
            }
        }
    }
    None
}

/// Classifies a default expression node's volatility.
fn classify_default_volatility(node: &Node) -> DefaultVolatility {
    let Some(inner) = node.node.as_ref() else {
        return DefaultVolatility::ConstantOrStable;
    };
    match inner {
        NodeEnum::AConst(_) | NodeEnum::SqlvalueFunction(_) => DefaultVolatility::ConstantOrStable,
        NodeEnum::TypeCast(tc) => tc
            .arg
            .as_deref()
            .map(classify_default_volatility)
            .unwrap_or(DefaultVolatility::ConstantOrStable),
        NodeEnum::AExpr(ae) => {
            let left = ae
                .lexpr
                .as_deref()
                .map(classify_default_volatility)
                .unwrap_or(DefaultVolatility::ConstantOrStable);
            if !matches!(left, DefaultVolatility::ConstantOrStable) {
                return left;
            }
            ae.rexpr
                .as_deref()
                .map(classify_default_volatility)
                .unwrap_or(DefaultVolatility::ConstantOrStable)
        }
        NodeEnum::AArrayExpr(arr) => {
            for elem in &arr.elements {
                let v = classify_default_volatility(elem);
                if !matches!(v, DefaultVolatility::ConstantOrStable) {
                    return v;
                }
            }
            DefaultVolatility::ConstantOrStable
        }
        NodeEnum::FuncCall(fc) => {
            let func_name = format_dotted_names(&fc.funcname);
            let bare_name = func_name
                .rsplit('.')
                .next()
                .unwrap_or(&func_name)
                .to_ascii_lowercase();

            if KNOWN_VOLATILE_FUNCTIONS.contains(&bare_name.as_str()) {
                return DefaultVolatility::Volatile(format!("{func_name}()"));
            }
            if KNOWN_NON_VOLATILE_FUNCTIONS.contains(&bare_name.as_str()) {
                for arg in &fc.args {
                    let v = classify_default_volatility(arg);
                    if !matches!(v, DefaultVolatility::ConstantOrStable) {
                        return v;
                    }
                }
                return DefaultVolatility::ConstantOrStable;
            }
            DefaultVolatility::Unknown(format!("{func_name}()"))
        }
        _ => DefaultVolatility::Unknown("complex expression".to_string()),
    }
}

/// Builds a finding for `ALTER TABLE ... ALTER COLUMN ... TYPE`.
fn alter_column_type_finding(
    table: &str,
    col_name: &str,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> HazardFinding {
    let lock = LockMode::AccessExclusive;
    HazardFinding {
        code: "PG002-TABLE-REWRITE".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] ALTER COLUMN {table}.{col_name} TYPE acquires {} lock and may trigger \
             a full table rewrite and index rebuild. \
             Limitation: static analysis cannot verify binary coercibility (pg_cast/pg_attribute) without target catalog metadata.",
            lock_mode_label(lock)
        ),
        severity: HazardSeverity::Critical,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::StaticallyDetected,
        remediation: Some(
            "Use an Expand-Contract pattern: add a new column, dual-write, backfill in batches, and swap."
                .to_string(),
        ),
    }
}

/// Checks `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY` for missing `NOT VALID`.
fn check_add_constraint(
    cmd: &AlterTableCmd,
    table: &str,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> Option<HazardFinding> {
    let def_node = cmd.def.as_deref()?;
    let Some(NodeEnum::Constraint(constraint)) = def_node.node.as_ref() else {
        return None;
    };
    check_fk_constraint(constraint, table, stmt_idx, offending_sql)
}

fn check_fk_constraint(
    constraint: &Constraint,
    table: &str,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> Option<HazardFinding> {
    if ConstrType::try_from(constraint.contype) != Ok(ConstrType::ConstrForeign) {
        return None;
    }
    if constraint.skip_validation {
        return None;
    }

    let ref_table = format_range_var(constraint.pktable.as_ref());
    let lock = LockMode::ShareRowExclusive;
    let con_label = if constraint.conname.is_empty() {
        "FOREIGN KEY".to_string()
    } else {
        format!("FOREIGN KEY ({})", constraint.conname)
    };

    Some(HazardFinding {
        code: "PG006-FK-MISSING-NOT-VALID".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] ADD CONSTRAINT {con_label} on {table} referencing {ref_table} without NOT VALID \
             acquires {} lock on both tables and scans all existing rows while holding the lock. \
             Limitation: scan duration depends on table row count.",
            lock_mode_label(lock)
        ),
        severity: HazardSeverity::High,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::StaticallyDetected,
        remediation: Some(
            "Add the constraint with NOT VALID, then run ALTER TABLE ... VALIDATE CONSTRAINT ... \
             in a subsequent statement (acquires SHARE UPDATE EXCLUSIVE)."
                .to_string(),
        ),
    })
}

/// Builds a finding for `ALTER TABLE ... DROP COLUMN` N-1 breakage.
fn drop_column_finding(
    table: &str,
    col_name: &str,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> HazardFinding {
    let lock = LockMode::AccessExclusive;
    HazardFinding {
        code: "PG007-DROP-COLUMN-N1-BREAKAGE".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] DROP COLUMN {table}.{col_name} acquires {} lock and breaks N-1 application \
             instances still querying the column (SQLSTATE 42703). \
             Limitation: static analysis cannot inspect live application queries; verify N-1 compatibility via PREPARE in simulation.",
            lock_mode_label(lock)
        ),
        severity: HazardSeverity::Critical,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::StaticallyDetected,
        remediation: Some(
            "Deploy application changes that stop reading/writing the column first, then drop it in a subsequent release."
                .to_string(),
        ),
    }
}

/// Checks `RENAME COLUMN` (and `RENAME TABLE`) for N-1 breakage under `ACCESS EXCLUSIVE`.
pub(crate) fn check_rename_stmt(
    stmt: &RenameStmt,
    stmt_idx: usize,
    offending_sql: Option<&str>,
) -> Option<HazardFinding> {
    let Ok(rename_type) = ObjectType::try_from(stmt.rename_type) else {
        return None;
    };
    if rename_type != ObjectType::ObjectColumn {
        return None;
    }

    let table = format_range_var(stmt.relation.as_ref());
    let lock = LockMode::AccessExclusive;
    Some(HazardFinding {
        code: "PG008-RENAME-COLUMN-N1-BREAKAGE".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] RENAME COLUMN {table}.{} TO {} acquires {} lock and breaks N-1 application \
             queries referencing the old column name (SQLSTATE 42703). \
             Limitation: static analysis cannot inspect live application queries; verify N-1 compatibility via PREPARE in simulation.",
            stmt.subname,
            stmt.newname,
            lock_mode_label(lock)
        ),
        severity: HazardSeverity::Critical,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::StaticallyDetected,
        remediation: Some(
            "Use an Expand-Contract pattern (add new column, dual-write/view, backfill, drop old column) instead of renaming in place."
                .to_string(),
        ),
    })
}

/// Returns an `UnsupportedScenario` finding for `DO $$ ... $$` blocks.
pub(crate) fn do_block_finding(stmt_idx: usize, offending_sql: Option<&str>) -> HazardFinding {
    HazardFinding {
        code: "PG099-DYNAMIC-DO-BLOCK".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] Dynamic DO block cannot be verified by static SQL AST inspection. \
             Limitation: PL/pgSQL control flow, dynamic EXECUTE, and lock acquisitions inside DO blocks are opaque to pg_query."
        ),
        severity: HazardSeverity::Medium,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::UnsupportedScenario,
        remediation: Some(
            "Replace dynamic DO blocks with explicit top-level SQL statements or verify in live container simulation."
                .to_string(),
        ),
    }
}

/// Returns an `UnsupportedScenario` finding for dynamic `EXECUTE` statements.
pub(crate) fn execute_stmt_finding(
    stmt_idx: usize,
    stmt_name: &str,
    offending_sql: Option<&str>,
) -> HazardFinding {
    HazardFinding {
        code: "PG098-DYNAMIC-EXECUTE".to_string(),
        summary: format!(
            "[stmt {stmt_idx}] Dynamic EXECUTE ({stmt_name}) cannot be verified by static SQL AST inspection. \
             Limitation: prepared/dynamic statement body and lock impact cannot be resolved statically."
        ),
        severity: HazardSeverity::Medium,
        offending_sql: offending_sql.map(str::to_string),
        status: VerificationStatus::UnsupportedScenario,
        remediation: Some(
            "Use explicit top-level DDL statements or verify in live container simulation.".to_string(),
        ),
    }
}

/// Returns a finding when `SET lock_timeout` is missing before DDL statements.
pub(crate) fn missing_lock_timeout_finding() -> HazardFinding {
    HazardFinding {
        code: "PG009-MISSING-LOCK-TIMEOUT".to_string(),
        summary: "Migration performs DDL without setting lock_timeout; if a table is busy, \
                  waiting for ACCESS EXCLUSIVE/SHARE locks will queue behind long queries and block all subsequent traffic. \
                  Limitation: static analysis only checks top-level SET lock_timeout in this script and cannot observe role/pooler defaults."
            .to_string(),
        severity: HazardSeverity::Medium,
        offending_sql: None,
        status: VerificationStatus::StaticallyDetected,
        remediation: Some("Add SET lock_timeout = '2s'; (or similar bounded timeout) before DDL statements.".to_string()),
    }
}

/// Returns a finding when `SET statement_timeout` is missing before DDL statements.
pub(crate) fn missing_statement_timeout_finding() -> HazardFinding {
    HazardFinding {
        code: "PG010-MISSING-STATEMENT-TIMEOUT".to_string(),
        summary: "Migration performs DDL without setting statement_timeout; unbounded DDL execution \
                  can hold locks or saturate I/O indefinitely. \
                  Limitation: static analysis only checks top-level SET statement_timeout in this script and cannot observe role/pooler defaults."
            .to_string(),
        severity: HazardSeverity::Medium,
        offending_sql: None,
        status: VerificationStatus::StaticallyDetected,
        remediation: Some("Add SET statement_timeout = '30s'; (or appropriate bound) before DDL statements.".to_string()),
    }
}
