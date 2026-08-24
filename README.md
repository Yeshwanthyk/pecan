# rust-template

Tight Rust lint template tuned for coding agents.

Source shape: audited against the Rust-specific policy patterns and heuristic rules shipped in `../slop-scan`.

## Goals

- fail fast on correctness and safety issues
- bias agents toward narrow, documented, library-friendly APIs
- explicitly deny selected allow-by-default Clippy lints for conversion safety, docs hygiene, async hazards, Cargo metadata, and ignored errors
- block common AI-generated Rust failure modes: `unwrap`, `expect`, broad `allow`, panic-driven control flow, clone-heavy ownership workarounds, undocumented `unsafe`, and sloppy public error surfaces
- give future Rust repos a copyable baseline instead of re-inventing lint policy

## Files

- `Cargo.toml` — workspace-level Rust + Clippy lint policy
- `.cargo/config.toml` — cargo aliases agents can run predictably
- `clippy.toml` — test-aware Clippy configuration and disallowed methods
- `rustfmt.toml` — deterministic formatting defaults
- `rust-toolchain.toml` — ensures `clippy` + `rustfmt` exist
- `deny.toml` — dependency hygiene policy via `cargo-deny`
- `slop-scan.config.json` — explicit Rust slop heuristics with non-production overrides
- `AGENTS.md` — authoring rules for coding agents before lint even runs
- `docs/rust-lint-policy.md` — detailed mapping from slop-scan findings to enforcement layers
- `.github/workflows/lint.yml` — CI example
- `scripts/lint.sh` — single local/CI entrypoint

## Use in another Rust repo

1. Copy these files into the target repo.
2. Keep a workspace root, even for single-crate repos.
3. Add this to every crate manifest:

```toml
[lints]
workspace = true
```

4. Adjust `members` in the root `Cargo.toml`.
5. If the project is binary-only or intentionally uses a different docs policy, relax specific lints deliberately instead of deleting the policy wholesale.

## Default validation loop

```bash
export RUSTFLAGS="-Dwarnings"
cargo fmt-check
cargo lint
cargo test-all
cargo deny-check
npx -y slop-scan scan . --lint
```

Or run everything through:

```bash
./scripts/lint.sh
```

## Why both Clippy and slop-scan?

Clippy catches mechanical Rust issues well. `slop-scan` covers repo-shape and AI-slop heuristics that Clippy cannot express cleanly, including:

- broad lint suppression
- unwrap / panic density in production code
- `anyhow` or `Result<T, String>` leaking through public APIs
- clone-heavy / string-owning ownership workarounds
- over-broad visibility and pass-through public wrappers
- undocumented or poorly bounded `unsafe`

## Template status

This repo includes `crates/template-lib` only so the template is self-validating. Real projects should rename or replace it.
