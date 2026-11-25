# Multi-Agent Asynchronous Workflow (Claude Opus & Hydrogen)

This specification governs how **Claude Opus** (Core Engineering) and **Hydrogen** (Product Engineering & Validation) collaborate asynchronously across separate Git worktrees without stepping on each other's code or breaking shared contracts.

---

## 1. Agent Roles & Worktree Isolation

| Agent | Role | Worktree / Branch Prefix | Owned Paths |
|---|---|---|---|
| **Claude Opus** | Core Engineer | `feat/core-*` | `crates/static-analyzer/`, `crates/simulator/`, `docs/security/` |
| **Hydrogen** | Product Engineer | `feat/product-*` | `crates/cli/`, `tests/`, `.github/`, report formatters, recipes |
| **Both (Cross-Review)** | Shared Boundary | Dedicated `feat/*-contracts-*` PR | `crates/contracts/` |

### Strict Worktree Rules
1. **Never commit to the other agent's branch prefix or owned paths.**
2. Each agent operates in its own isolated Git worktree (e.g., `../migrationx-core` on `feat/core-*` and `../migrationx-product` on `feat/product-*`) and rebases onto `main` independently.
3. **Mock-First Unblocking:** Hydrogen develops `crates/cli` and report renderers against `tests/fixtures/mocks/` (`sample_snapshot.json`, `sample_static_findings.json`, `sample_runtime_verdict.json`) without waiting for `static-analyzer` or `simulator` engine implementations.

---

## 2. Contract-Change PR Protocol (`crates/contracts`)

`crates/contracts` is the compile-time boundary between Core and Product Engineering:

1. **Isolation:** Any change to `crates/contracts` must be isolated in its own focused PR (do not bundle engine or CLI feature work into a contract PR).
2. **Mutual Approval:** `.github/CODEOWNERS` requires **both** `@core-engineer` and `@product-engineer` to approve any PR touching `/crates/contracts/`.
3. **Versioning & Compatibility:**
   - Breaking schema changes require bumping `SCHEMA_VERSION` in `crates/contracts/src/schema.rs`.
   - Update `tests/fixtures/mocks/*.json` in the same PR (or immediately follow up) so contract deserialization tests remain green.
   - `VerificationStatus` must always preserve the distinction between `StaticallyDetected`, `ObservedInSimulation`, `CouldNotVerify`, `Passed`, and `UnsupportedScenario`.
4. **Green Workspace Gate:** All workspace crates must compile and pass `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` before merge.

---

## 3. 48–72 Hour Merge Cadence

- Feature branches **must merge to `main` every 48–72 hours** to prevent schema drift and integration debt.
- If a branch exceeds **72 hours** without merging:
  1. Pause new feature work on that branch.
  2. Rebase onto `main` and run the full workspace test suite + deterministic fixture gate.
  3. Split or land the stabilized subset before continuing.
- **Deterministic Fixture Gate:** No AI/LLM-assisted or non-deterministic heuristics may be merged until all deterministic top-20 PostgreSQL migration hazard fixtures pass in CI.
