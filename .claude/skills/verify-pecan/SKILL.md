---
name: verify-pecan
description: Launch, drive, and prove Pecan (Rust server + React phone-first web UI over Pi RPC sessions) against a deterministic fixture model. Use when verifying or hill-climbing any Pecan session feature through the CLI or the phone-viewport browser.
---

# verify-pecan

Pecan serves a web UI over Pi (`pi --mode rpc`) sessions. Every flow is
drivable from the `pecan` CLI; the browser smoke covers the phone UI. Runs use
an isolated agent dir and the scripted fixture model, so they are fast,
deterministic, and never read or write `~/.pi`.

## Launch

One command builds, launches an isolated server, drives everything, and tears
down:

```bash
scripts/pecan-check.sh --keep --ui --out /tmp/pecan-scorecard.json
```

To drive by hand, reproduce what the script sets up:

```bash
cargo build -p pecan
SCRATCH="$(mktemp -d)"; mkdir -p "$SCRATCH/agent/sessions" "$SCRATCH/project"
export PECAN_AGENT_DIR="$SCRATCH/agent"
export PECAN_PI_ARGS="--no-extensions --extension $PWD/scripts/fixture-model.ts --provider pecan-fixture --model fixture-v1"
export PECAN_SERVER_URL="http://127.0.0.1:4799"
target/debug/pecan add "$SCRATCH/project"
target/debug/pecan serve --port 4799 --no-open > "$SCRATCH/server.log" 2>&1 &
```

Ready when `$PECAN_AGENT_DIR/pecan/cli-token` exists and `target/debug/pecan health --json`
succeeds. API calls need auth: the CLI reads that token itself; for curl use
`AUTH=(-H "Authorization: Bearer $(cat $PECAN_AGENT_DIR/pecan/cli-token)")`.
Web changes need `cd web && npm run build` first — the binary embeds `web/dist`.

## Doctor

Read-only, before driving:

- `command -v pi jq cargo node` all resolve; `--ui` also needs Google Chrome.
- `target/debug/pecan health --json` gives `.status == "ok"` with a `version`
  (the server can run `pi`).
- `echo $PECAN_AGENT_DIR` points into a scratch dir, **never** `~/.pi/agent`
  (without it, Pecan and Pi default to the real sessions).
- `curl -fsS "${AUTH[@]}" $PECAN_SERVER_URL/api/bootstrap | jq '.projects | length'` ≥ 1.
- `target/debug/pecan session new --cwd "$SCRATCH/project" --json` returns an
  `id`, and `session send <id> hi --json` gives `.turn.text == "Echo: hi"` —
  this proves the fixture model, not a real provider, is answering.

## Drive

- **CLI**: `pecan session new|send|wait|respond|abort|state|asks` and
  `pecan events [--session <id>]`, `pecan health`. Add `--json` for machine output and
  `--timeout <secs>` to bound waits. `send` subscribes to SSE before posting
  and returns a `turn` report once the agent settles or asks the user.
- **Fixture prompts** (scripts/fixture-model.ts): plain text echoes;
  `/slow`, `/tool`, `/confirm`, `/think`, `/rich`, `/error` script the other
  paths.
- **Browser**: `node web/scripts/ui-smoke.mjs --url $PECAN_SERVER_URL --pair "$(target/debug/pecan pair --json | jq -r .code)" --out DIR`
  (the first step proves the pair screen and pairs through it)
  drives a 390×844 touch viewport via stable `data-testid`s
  (`empty-new-session`, `composer-input`, `send`, `abort`, `working`,
  `thread`, `turn-error`, `turn-stopped`, `mermaid[data-state]`, `toast`).

## Evidence

- Scorecard JSON: `{pass, passed, total, scenarios:[{name, pass, ms, …}]}`;
  each CLI scenario keeps its raw response in the scratch dir.
- Turn reports carry `outcome`, `firstTokenMs`, `totalMs`, `text`, `tools`,
  `error`, `dialog`.
- UI: `<scratch>/ui/ui-scorecard.json` (per-step ms, `initialScriptBytes` = JS fetched before first paint, `loadedScriptBytes` incl. idle prefetch,
  `overflowPx`, console errors) plus one PNG per step.
- Side effects: the session JSONL under `$PECAN_AGENT_DIR/sessions/` and a
  second read via `pecan thread <id>` or `GET /api/thread/<id>`.

## Cleanup

`pecan-check.sh` kills the server it started. With `--keep`, the scratch dir
(server log, step JSON, screenshots) is kept and its path is printed; delete it
once the evidence has been reviewed. For a manual launch, `kill %1` the server
and remove `$SCRATCH`. Never touch `~/.pi`.

## Features

See [features/README.md](features/README.md) for the feature index and one
recipe per feature.
