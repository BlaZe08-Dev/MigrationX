# 03 — PostgreSQL Safety

## Lock Compatibility

- **Know the lock matrix.** Every `ALTER TABLE`, `CREATE INDEX`, `DROP`, `VACUUM`,
  and `REINDEX` statement must be annotated with its required lock mode.
- `ACCESS EXCLUSIVE` blocks **all** concurrent operations — this is the default
  for most DDL and must always be flagged as `Critical`.
- `SHARE UPDATE EXCLUSIVE` (used by `CREATE INDEX CONCURRENTLY`) is compatible
  with `ROW EXCLUSIVE` (normal DML) but blocks other schema changes.

## Table Rewrites

The following operations trigger a full table rewrite and must be flagged:
- Changing a column's data type (unless to a binary-compatible type).
- Adding a column with a `DEFAULT` that is not a constant (before PG 11) or
  with a volatile default.
- `ALTER TABLE ... SET NOT NULL` without an existing valid `CHECK` constraint (PG < 12).
- Adding a primary key to a table without one.

## `CONCURRENTLY` Rules

- `CREATE INDEX CONCURRENTLY` **cannot** run inside a transaction block.
- `DROP INDEX CONCURRENTLY` **cannot** run inside a transaction block.
- `REINDEX CONCURRENTLY` requires PostgreSQL 12+.
- If a concurrent index build fails, it leaves an `INVALID` index that must be
  explicitly dropped.

## Transaction Boundaries

- Multi-statement migrations must document their transaction boundary strategy.
- Long-running transactions hold locks for their entire duration — flag any
  migration that combines DDL with long-running DML in a single transaction.
- Advisory locks do not appear in `pg_locks` by default — document this limitation.

## Version-Specific Behavior

All findings must note the PostgreSQL version range they apply to:
- PG 11+: `ADD COLUMN ... DEFAULT` with a constant no longer rewrites the table.
- PG 12+: `SET NOT NULL` can use an existing valid `CHECK` constraint.
- PG 14+: `REINDEX` supports `TABLESPACE` clause.
- PG 15+: `MERGE` statement support.

## Read-Only Analysis

The static analyzer and snapshot collector **never** write to the target database.
All connections to production databases must use `SET default_transaction_read_only = on`.
