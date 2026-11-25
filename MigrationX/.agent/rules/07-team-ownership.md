# 07 — Team Ownership

## Two-Person Team Model

### Core Engineer

**Owns:**
- `crates/static-analyzer` — SQL parsing, hazard detection rules.
- `crates/simulator` — Container lifecycle, lock observation, runtime verification.
- Security architecture and threat model.
- Crate dependency DAG integrity.
- PostgreSQL correctness (lock modes, rewrite rules, version-specific behavior).

**Reviews but does not own:**
- `crates/contracts` — changes require cross-review approval.

### Product Engineer

**Owns:**
- `crates/cli` — Argument parsing, output formatting, user experience.
- Snapshot collector tooling.
- Report generation and formatting.
- Migration recipe library.
- Test fixtures and deterministic test data.
- CI/CD pipeline configuration.

**Reviews but does not own:**
- `crates/contracts` — changes require cross-review approval.

## Cross-Review Protocol for `crates/contracts`

`crates/contracts` is the **shared boundary** between both engineers. Modifications
require:

1. A PR authored by either engineer.
2. **Explicit approval from the other engineer** before merge.
3. All downstream crates must compile and pass tests after the change.
4. Schema version bump if the change is breaking.

## Escalation

If a disagreement arises on a `contracts` change:
1. Both engineers document their position in the PR.
2. The change is evaluated against the phased workflow (does it meet the
   principal engineering bar?).
3. If unresolved, the change is deferred until a design doc is written and
   agreed upon.

## Accountability

- Each engineer is responsible for the test coverage of their owned crates.
- Security-sensitive changes (credential handling, container isolation, read-only
  enforcement) require Core Engineer review regardless of which crate is modified.
- CI pipeline changes require Product Engineer review even if triggered by a
  Core Engineer's crate change.
