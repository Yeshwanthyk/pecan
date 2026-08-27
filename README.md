# Pecan

Pecan is a local browser UI for [Pi](https://github.com/badlogic/pi-mono) coding-agent sessions. It keeps Pi authoritative: Pecan launches the normal `pi --mode rpc` worker, reads Pi's JSONL sessions, and renders a web UI over the same local data.

## What works

- browse projects, sessions, and transcripts
- send, steer, queue, abort, resume, and switch Pi models
- stream assistant text and tool calls
- answer Pi extension dialogs (`select`, `confirm`, `input`, `editor`)
- render Pi extension notices, statuses, widgets, and editor-prefill updates
- show `pi-subagents` child activity and persisted child sessions
- show `pi-tasks` task lists and active task state
- show `pi-workflows` runs, phases, agents, artifacts, and errors
- use `/workflow-draft` through an RPC-safe editor/confirmation flow

Pecan uses your existing Pi installation and `~/.pi/agent/settings.json` by default, so installed Pi extensions continue to be discovered by Pi. For an explicit extension entry point during development, set `PECAN_PI_EXTENSIONS` to absolute paths separated by commas or whitespace.

## Run

```bash
cargo run -p pecan -- serve
```

Useful options:

```text
pecan serve --no-open
pecan serve --host 127.0.0.1 --port 7615
pecan serve --session <id>
pecan serve --session latest --cwd <absolute-cwd>
```

The server is local-only by default and serves the built frontend from `web/dist`.

## Verify

```bash
cargo test-all
cd web && npm run lint && npm run build
cd ../pi-workflows && npm run check && npm test
```

The extension bridge proof is documented in [`docs/extension-dialog-proof.md`](docs/extension-dialog-proof.md), with deterministic scripts under `scripts/`.

## Scope

Pecan intentionally supports Pi's portable RPC UI surface rather than trying to load arbitrary TypeScript extensions into Rust or the browser. Extensions that rely on Pi's TUI-only `ctx.ui.custom()` need an RPC-safe fallback in the extension; `pi-workflows` now provides that fallback for draft review. Pi remains responsible for executing extension code and persisting its state.
