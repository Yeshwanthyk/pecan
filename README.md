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
Set `PECAN_AGENT_DIR` to point Pecan (and the Pi workers it spawns) at a different agent directory.

## Pairing

Every API call and event stream needs a credential. Loopback is not trusted,
because a proxy such as `tailscale serve` forwards remote phones from 127.0.0.1.

- **CLI:** `pecan serve` writes a secret to `<agent dir>/pecan/cli-token`
  (mode 0600). The CLI sends it as `Authorization: Bearer`; `PECAN_TOKEN`
  overrides it.
- **Browsers and phones:** run `pecan pair` and either open the printed
  `…/?pair=CODE` link or type the code on the pair screen. The browser gets an
  `HttpOnly`, `SameSite=Strict` device cookie, which is also `Secure` behind
  https.
  - Codes are 8 characters, single use, and expire after 10 minutes.
  - After 10 wrong guesses, every live code is dropped.
  - `pecan serve` without `--no-open` pairs the browser it opens.
- **Devices:** `pecan devices` lists paired devices. `pecan devices revoke <id>`
  signs a device out at once and closes its live stream.

### Reach it from a phone over Tailscale

`pecan remote [--port <n>]` prints the tailnet URL
(`https://<machine>.<tailnet>.ts.net:<port>`) and whether `tailscale serve`
already proxies that HTTPS port to Pecan. It changes nothing on its own.

- `pecan remote --enable` runs
  `tailscale serve --bg --https=<port> http://127.0.0.1:<port>`.
- It refuses when that port already proxies to another service.
- Then pair the phone with `pecan pair --url <that url>`.
- Stop serving with `tailscale serve --https=<port> off`.

### Push notifications

Turn on **Settings → Notifications** on a phone and it subscribes to web push.
On iPhone this needs the Home Screen app, opened over HTTPS, such as the
Tailscale URL.

- When a turn finishes or Pi asks for input, Pecan pushes to every subscribed
  device that has no live event stream. An open tab already shows its own
  notification, so it is not pushed.
- Pushes carry no content. The service worker fetches the text from
  `/api/push/pending` with the device cookie, so the push service never sees
  session titles.
- Endpoints must belong to a known push service (Google, Mozilla, Apple or
  Microsoft).
- The VAPID key lives in `<agent dir>/pecan/vapid-key` (mode 0600).
  `PECAN_PUSH_SUBJECT` sets the contact claim.
- `pecan push` lists subscriptions. `pecan push test` sends a test push and
  reports each device's outcome; subscriptions the push service reports gone
  are dropped.

## Drive sessions from the CLI

Every session flow the UI offers is scriptable against a running server
(`--server <url>` or `PECAN_SERVER_URL`, default `http://127.0.0.1:7614`):

```text
pecan session new --cwd <dir>
pecan session send <id> <text> [--mode send|steer|queue] [--no-wait]
pecan session wait <id>
pecan session asks <id>
pecan session respond <id> <ask-id> --confirm | --deny | --cancel | --value <v>
pecan session abort <id>
pecan session state <id>
pecan events [--session <id>]
pecan health [--refresh]
pecan pair [--url <phone-facing base>]
pecan devices [revoke <id>]
pecan remote [--port <n>] [--enable]
pecan push [test]
```

`--json` prints a turn report (`outcome`, `firstTokenMs`, `totalMs`, `text`,
`tools`, `error`, `dialog`); `--timeout <secs>` bounds each wait.

Built for flaky phone links:

- **Health.** `pecan health` (`GET /api/health`) reports whether the server can
  run `pi` and which version; the UI shows a banner when it cannot.
  `--refresh` re-probes instead of using the cached result.
- **Retry-safe sends.** `session new`, `send` and `respond` accept
  `--idempotency-key <key>` (8–128 chars; the UI sends an `Idempotency-Key`
  header on every mutation). A retry with the same key returns the first
  result for 10 minutes instead of sending twice; failures are not remembered.
- **Resumable events.** `GET /api/events?after=<seq>` (or `Last-Event-ID`)
  replays every frame after `seq`, then continues live. If the gap is gone
  (server restart or too old) the stream sends `reset` and the client refetches.
- **Attention.** While the tab is hidden, a finished turn or a waiting dialog
  badges the title, plays a chime, and (if enabled in Settings) shows a system
  notification.

## Verify

```bash
scripts/pecan-check.sh --ui
```

This builds Pecan, starts an isolated server with the scripted fixture model
(`scripts/fixture-model.ts`, never touching `~/.pi`), drives every session flow
through the CLI, runs a phone-viewport browser smoke (`web/scripts/ui-smoke.mjs`,
needs Google Chrome), and prints a JSON scorecard with timings. Use `--keep` to
keep the server log and screenshots, `--out <file>` to save the scorecard. The
verification recipes live in `.claude/skills/verify-pecan/`.

Repository checks:

```bash
cargo fmt-check && cargo lint && cargo test-all && cargo deny-check
npx -y slop-scan scan . --lint --ignore "web/dist/**"
cd web && npm run lint && npm run build
```

`web/dist` is the generated bundle, so slop-scan skips it.

The extension bridge proof is documented in [`docs/extension-dialog-proof.md`](docs/extension-dialog-proof.md), with deterministic scripts under `scripts/`.

## Scope

Pecan intentionally supports Pi's portable RPC UI surface rather than trying to load arbitrary TypeScript extensions into Rust or the browser. Extensions that rely on Pi's TUI-only `ctx.ui.custom()` need an RPC-safe fallback in the extension; `pi-workflows` now provides that fallback for draft review. Pi remains responsible for executing extension code and persisting its state.
