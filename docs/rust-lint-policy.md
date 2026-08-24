# Rust lint policy

This template turns the Rust-specific `slop-scan` rule set into a reusable project policy for coding agents.

## Design

Use multiple enforcement layers instead of pretending one tool can express everything:

1. `rustfmt` — formatting determinism
2. compiler lints — baseline safety, visibility, and docs discipline
3. Clippy — mechanical API and control-flow constraints
4. `cargo-deny` — dependency hygiene
5. `slop-scan` — heuristic AI-slop and repo-shape checks
6. `AGENTS.md` — pre-generation guidance for coding agents

## What came from `slop-scan`

Audited Rust rules in `../slop-scan`:

- `rust.allow-broad-lints`
- `rust.allow-proliferation`
- `rust.box-dyn-over-generics`
- `rust.clone-before-consume`
- `rust.clone-density`
- `rust.derives-with-interior-mutability`
- `rust.error-message-template-repetition`
- `rust.expect-generic-message`
- `rust.god-functions`
- `rust.match-propagation-boilerplate`
- `rust.over-derive`
- `rust.panic-in-library`
- `rust.pub-mod-glob-reexport`
- `rust.public-anyhow-leak`
- `rust.public-api-churn-shape`
- `rust.public-missing-docs-cluster`
- `rust.public-result-string-error`
- `rust.question-mark-avoidance`
- `rust.repetitive-panic-messages`
- `rust.restating-comments`
- `rust.silenced-result`
- `rust.string-over-borrow`
- `rust.todo-panic-in-match-default`
- `rust.unit-error-type-overuse`
- `rust.unreachable-macro-production`
- `rust.unsafe-undocumented`
- `rust.unsafe-without-safe-wrapper-boundary`
- `rust.unwrap-density`
- `rust.visibility-discipline`

## Mapping: slop-scan -> template policy

| slop-scan theme | Enforced by | Template choice |
| --- | --- | --- |
| broad `allow(...)` use | Clippy + slop-scan | `clippy::allow_attributes`, `clippy::allow_attributes_without_reason`, plus `rust.allow-broad-lints` / `rust.allow-proliferation` |
| unwrap / expect / panic driven code | Clippy + slop-scan | deny `unwrap_used`, `expect_used`, `panic`, `panic_in_result_fn`, plus `rust.unwrap-density`, `rust.expect-generic-message`, `rust.panic-in-library` |
| sloppy public API errors | compiler + Clippy + slop-scan | deny `missing_docs`, `missing_errors_doc`, `missing_safety_doc`, plus `rust.public-anyhow-leak`, `rust.public-result-string-error`, `rust.unit-error-type-overuse` |
| undocumented / exposed `unsafe` | compiler + Clippy + slop-scan | deny `unsafe_code`, `unsafe_op_in_unsafe_fn`, `undocumented_unsafe_blocks`, plus `rust.unsafe-undocumented`, `rust.unsafe-without-safe-wrapper-boundary` |
| clone-heavy ownership workarounds | Clippy + slop-scan | deny `redundant_clone`, `clone_on_ref_ptr`, plus `rust.clone-density`, `rust.clone-before-consume`, `rust.string-over-borrow` |
| overly broad visibility / noisy public surface | compiler + slop-scan | deny `unreachable_pub`, plus `rust.visibility-discipline`, `rust.pub-mod-glob-reexport`, `rust.public-api-churn-shape` |
| boilerplate fallible control flow | Clippy + slop-scan | prefer `?`; backstop with `rust.match-propagation-boilerplate` and `rust.question-mark-avoidance` |
| over-commented / low-signal prose | slop-scan | `rust.restating-comments`, `rust.error-message-template-repetition`, `rust.repetitive-panic-messages` |
| large or over-derived shapes | slop-scan | `rust.god-functions`, `rust.over-derive`, `rust.derives-with-interior-mutability` |

## Why non-production overrides exist

`slop-scan` itself notes that Rust heuristics need path context. Tests, examples, benches, fuzz targets, and binaries often tolerate patterns that would be poor defaults in library code.

This template therefore keeps strict Clippy rules globally, but relaxes selected `slop-scan` heuristics for:

- `tests/**`
- `benches/**`
- `examples/**`
- `fuzz/**`
- `src/bin/**`

That keeps production library code tight without making support code miserable.

## Opinionated defaults

### Compiler lints

- deny safety issues: `unsafe_code`, `unsafe_op_in_unsafe_fn`
- deny public-surface sloppiness: `missing_docs`, `unreachable_pub`
- deny ignored failures: `unused_must_use`
- deny stale type noise: `unused_lifetimes`, `unused_qualifications`

### Clippy lints

- deny runtime escape hatches: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`
- deny observability leaks: `print_stdout`, `print_stderr`, `dbg_macro`
- deny sloppy indexing/string slicing: `indexing_slicing`, `string_slice`
- deny poor error docs and unsafe docs: `missing_errors_doc`, `missing_panics_doc`, `missing_safety_doc`, `undocumented_unsafe_blocks`
- deny weak namespace hygiene: `wildcard_imports`, `enum_glob_use`
- deny ownership shortcuts: `redundant_clone`, `clone_on_ref_ptr`
- deny selected allow-by-default Clippy lints with high signal for agent-written code:
  conversion safety (`as_conversions`, `checked_conversions`), docs
  correctness (`doc_broken_link`, `doc_link_with_quotes`, `doc_markdown`),
  async/stack hazards (`future_not_send`, `large_futures`,
  `large_stack_arrays`, `large_stack_frames`, `unused_async`), API/error
  shape (`fallible_impl_from`, `ref_option`, `unnecessary_wraps`,
  `unwrap_in_result`), ignored failure paths (`let_underscore_must_use`,
  `unused_result_ok`), test quality (`missing_assert_message`,
  `tests_outside_test_module`), Cargo hygiene (`cargo_common_metadata`,
  `negative_feature_names`, `redundant_feature_names`,
  `wildcard_dependencies`), concurrency/data-structure footguns
  (`mutex_atomic`, `mutex_integer`, `needless_collect`), and namespace/text
  hygiene (`disallowed_script_idents`, `non_ascii_literal`,
  `redundant_pub_crate`, `used_underscore_binding`)
- do not enable `clippy::restriction`, `clippy::pedantic`, `clippy::nursery`,
  or `clippy::cargo` wholesale; cherry-pick individual lints so policy changes
  stay reviewable

### Clippy configuration

- keep lint levels in `Cargo.toml`, because Clippy config files tune behavior but
  do not allow or deny lints
- set `msrv` in `clippy.toml` to match the workspace `rust-version`
- set `avoid-breaking-exported-api = false` so public API issues are still
  visible in this template instead of being hidden by compatibility heuristics
- enable Clippy's stricter constructor/comment checks with
  `check-inconsistent-struct-field-initializers` and `lint-commented-code`

## Adoption notes

### Every crate must opt in

Workspace lints do nothing unless each crate manifest contains:

```toml
[lints]
workspace = true
```

### Adjust policy surgically

If a repo needs a deviation:

- allow one specific lint, not a whole category
- keep the scope local, not crate-wide
- document the reason in code
- prefer path-scoped `slop-scan` overrides over disabling the rule globally

### Recommended CI order

```bash
export RUSTFLAGS="-Dwarnings"
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
npx -y slop-scan scan . --lint
```

## Tighten later

Likely next iterations:

- add a stricter profile for `clippy::pedantic` / selected `clippy::nursery`
- add repo-specific `disallowed-types`
- add architectural `slop-scan` overrides per workspace area
- add package-template generators so new crates inherit `[lints] workspace = true` automatically
