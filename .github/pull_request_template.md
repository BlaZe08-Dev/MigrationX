## Summary

<!-- Briefly describe what this PR changes and why. -->

## Worktree & Ownership Checklist

- [ ] **Worktree scope respected:** Changes are confined to my owned paths (`feat/core-*` for `static-analyzer`, `simulator`, security; `feat/product-*` for `cli`, `tests`, recipes, `.github`).
- [ ] **`crates/contracts` untouched** — OR, if modified, explicitly flagged for **cross-review approval** from both Core and Product Engineers (with `SCHEMA_VERSION` bumped if breaking).
- [ ] **Zero production data or secrets:** No customer row data, credentials, or unredacted connection strings are included in code, fixtures, or logs.

## Verification

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Deterministic fixture gate remains green
