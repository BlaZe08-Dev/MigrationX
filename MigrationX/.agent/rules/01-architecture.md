# 01 — Architecture

## Crate Dependency DAG

```
contracts  ←  static-analyzer  ←  cli
contracts  ←  simulator         ←  cli
```

### Rules

1. **`contracts`** is the leaf crate — it depends on nothing within the workspace.
2. **`static-analyzer`** depends only on `contracts`.
3. **`simulator`** depends only on `contracts`.
4. **`cli`** depends on `static-analyzer`, `simulator`, and `contracts`.
5. **Zero circular dependencies** — enforced by Cargo and CI.

## Crate Responsibilities

| Crate | Owns | Does NOT own |
|---|---|---|
| `contracts` | Versioned schemas, enums, serialization | Business logic, I/O, CLI flags |
| `static-analyzer` | SQL parsing, hazard detection rules | Container management, CLI output |
| `simulator` | Container lifecycle, lock observation, broken query detection | SQL parsing rules, CLI output |
| `cli` | Argument parsing, output formatting, orchestration | Business logic, hazard rules |

## No Business Logic in `cli`

The `cli` crate is a **thin orchestration layer**. It:
- Parses arguments and configuration.
- Calls into `static-analyzer` and `simulator`.
- Formats and presents results.

It does **not** implement hazard detection, lock analysis, or any domain logic.

## Shared Types

All cross-crate data flows through `contracts` types. No ad-hoc structs, no
`serde_json::Value` as a contract boundary, no `String`-typed enums.
