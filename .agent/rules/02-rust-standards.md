# 02 — Rust Standards

## Idiomatic Rust

- Use `rustfmt` defaults — no custom `rustfmt.toml` overrides.
- Prefer iterators over index loops.
- Use `impl Trait` in argument position when appropriate.
- Prefer `&str` over `String` in function parameters unless ownership is needed.

## Error Handling

- **Library crates** (`contracts`, `static-analyzer`, `simulator`): use `thiserror` for
  typed, domain-specific errors. Every error variant must carry enough context to
  diagnose the issue without a debugger.
- **Binary crate** (`cli`): use `anyhow` for top-level error propagation.
- **No `.unwrap()` or `.expect()` on production paths.** These are allowed only in:
  - Test code (`#[cfg(test)]`).
  - `const` / `static` initialization where the value is compile-time known.
- Use `?` operator for all fallible operations.

## No `unsafe`

- Zero `unsafe` blocks in the entire workspace.
- If a dependency requires `unsafe` internally, document why it's acceptable in
  the crate's `lib.rs` module-level docs.

## Async Discipline

- All async functions must have **explicit timeouts** (via `tokio::time::timeout`).
- Document cancellation safety for every `async fn` that holds resources across
  `.await` points.
- Prefer `tokio::select!` with explicit cancellation branches over bare `.await`.

## Linting

- CI runs: `cargo clippy --workspace --all-targets -- -D warnings`
- All `#[allow(...)]` attributes require a justifying comment.
- No `#[allow(clippy::all)]` or blanket suppression.

## Documentation

- Every public item has a doc comment (`///`).
- Module-level docs (`//!`) for every `lib.rs` and `mod.rs`.
- Examples in doc comments for key public functions.
