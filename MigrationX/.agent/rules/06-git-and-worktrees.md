# 06 — Git & Worktrees

## Branch Strategy

### Worktree Isolation

Two parallel worktrees with **strict ownership boundaries**:

| Worktree | Branch Prefix | Owns |
|---|---|---|
| Core | `feat/core-*` | `static-analyzer`, `simulator`, security, architecture |
| Product | `feat/product-*` | `cli`, snapshot collector, reports, recipes, fixtures, CI |

### Rules

1. **Never** commit to the other worktree's branch prefix.
2. Changes to `crates/contracts` require a PR reviewed by **both** worktrees' owners.
3. Each worktree rebases onto `main` independently.

## Merge Cadence

- Merge feature branches into `main` every **48–72 hours**.
- If a branch exceeds 72 hours without merging, it must be rebased and
  integration-tested before any new work begins.
- Prefer small, focused PRs over long-lived feature branches.

## Deterministic Fixture Gate

Before adding any AI/LLM-powered features, non-deterministic analysis, or
heuristic-based detection:

1. **All deterministic fixtures must be in place and passing.**
2. The core hazard detection rules must work with hard-coded SQL patterns.
3. Test fixtures must cover the top 20 most common PostgreSQL migration hazards.
4. Only after the deterministic baseline is green may non-deterministic features
   be introduced — and they must have their own isolated test suite with
   tolerance thresholds.

## Commit Messages

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`
Scopes: `contracts`, `static-analyzer`, `simulator`, `cli`, `infra`
