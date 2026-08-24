# AGENTS.md

Rust repo policy for coding agents.

## Outcome bias

Write code that passes the workspace lint policy without needing broad suppressions.

## Hard rules

- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, or `unreachable!` in production code.
- No `#[allow(...)]` unless the lint is narrowly scoped and the reason is written inline.
- Do not add `#[allow(dead_code)]` in normal code. Delete, move behind a feature, or make the item reachable.
- No `pub use module::*` in public API surfaces.
- No `anyhow::Result`, `Box<dyn Error>`, or `Result<T, String>` in public library APIs.
- No undocumented `unsafe`. Every unsafe block needs a `// SAFETY:` explanation. Prefer a safe wrapper boundary over exposing `pub unsafe fn` directly.
- Prefer borrowed inputs (`&str`, slices, references) over owned `String` / `Vec` when ownership is not required.
- Avoid clone-driven ownership fixes. Reshape lifetimes, borrowing, or data flow first.
- Prefer `?` over verbose `match Ok/Err` pass-through code.
- Keep visibility narrow. Default to private, then `pub(crate)`, then `pub` only when needed.
- Public items need doc comments.
- Do not manually edit generated files, snapshots, or checked-in query caches. Change the source input and regenerate.

## Rust shape

- Use the workspace toolchain, MSRV, edition, package metadata, and lint policy. New crates must inherit workspace metadata and include `[lints] workspace = true`.
- Keep a single workspace root unless the user explicitly asks for a nested workspace.
- When adding, deleting, renaming, or moving crates, update workspace membership plus CI, packaging, Docker, release, and generated manifests that reference crate paths.
- Avoid byte indexing or slicing `String` / `&str`. Use UTF-8-safe APIs such as `chars`, `char_indices`, `str::get`, or a local char-boundary helper.
- Keep modules small and cohesive. Prefer explicit interfaces over globals, ambient state, or pass-through public wrappers.

## Clippy policy

- Keep lint levels in `Cargo.toml`; use `clippy.toml` only for behavior and threshold tuning.
- Do not enable `clippy::restriction`, `clippy::pedantic`, `clippy::nursery`, or `clippy::cargo` wholesale. Add individual lints with a clear reason.
- When Clippy flags a false positive, trace references, borrow/move flow, and macro expansion before suppressing it.
- Prefer fixing the root cause over adding `#[allow(...)]`. If an allow is required, scope it to the smallest item and explain why inline.
- When a project rule repeats in review, encode it with a Clippy lint level, `disallowed-methods`, `disallowed-types`, `disallowed-macros`, or another automated check.

## Error and API design

- Library crates should expose typed domain errors, usually with `thiserror`; binaries may use `anyhow` internally at the top-level orchestration boundary.
- Preserve error source/context when crossing boundaries. Prefer typed constructors or structured variants over stringly internal errors.
- Validate external input at the boundary, then pass typed values inward. Avoid keyword/regex heuristics for policy, routing, auth, or state decisions when structured data can represent the decision.
- Model lifecycle/state transitions explicitly with enums and checked transition functions when invalid state changes are possible.
- Escape or encode untrusted data when generating protocol text such as XML, HTML, shell snippets, or SQL.

## Async, process, and observability

- Use bounded channels by default. Unbounded channels need a local justification.
- Put explicit timeouts around external I/O, network calls, subprocesses, and long-running waits.
- Do not leave detached background tasks without cancellation, shutdown, and error-reporting paths.
- Use `tracing` for runtime diagnostics with structured fields. Do not use `println!`, `eprintln!`, or `dbg!` in library/application code.
- Preserve the repository's supervisor/process model. Do not add ad-hoc workers, watchers, or process-kill flows when the repo already has a managed entrypoint.

## Data and migrations

- Prefer compile-time checked SQL for static queries when the repo uses SQLx (`query!`, `query_as!`, `query_scalar!`).
- Keep migration files forward-only by default. Do not put rollback-only `DROP` statements in a single up migration; use explicit up/down migration pairs when rollback is required.
- For tables with `updated_at`, update it in every mutation that changes persistent state.
- After SQL query or migration changes, refresh and commit checked-in SQLx metadata when the repo uses offline SQLx mode.

## Tests

- Non-trivial behavior changes need tests for both success and failure paths.
- Mock external services and network dependencies in tests. Do not make unit/integration tests depend on live third-party services unless the repo has a dedicated live-test gate.
- For async or state-machine code, test cancellation/closed-channel/error-transition behavior, not only the happy path.

## Before finishing

Run:

```bash
cargo fmt-check
cargo lint
cargo test-all
cargo deny-check
npx -y slop-scan scan . --lint
```

## Allowed exceptions

- Tests, benches, examples, fuzz targets, and binary entrypoints may be slightly looser, but still prefer the same style.
- If a lint must be suppressed, keep the scope minimal and explain why directly next to the suppression.
