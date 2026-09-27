# Sessions

A user starts a Pi session in a project, sends a message, and sees the reply stream in.

## Behaviors

- S1 create: a new session gets an id and opens with an empty composer.
- S2 send: a message produces a streamed assistant reply.
- S3 read back: the turn is persisted and visible on reload.

## User entry points

- CLI: `pecan session new --cwd <dir>` then `pecan session send <id> <text>`.
- Phone UI: empty state project button (`empty-new-session`).
- Desktop UI: per-project + in the sidebar tree (`project-new-session`) and header button
  (`header-new-session`) — not driven by the smoke.

## Drive

1. `pecan session new --cwd "$SCRATCH/project" --json` → `.id` non-empty.
2. `pecan session send <id> "hello pecan" --json` → `.turn.outcome == "settled"`,
   `.turn.text == "Echo: hello pecan"`, `.turn.firstTokenMs` present.
3. `pecan thread <id>` → shows the user message and the echo.
4. UI: smoke steps `new-session`, `send-echo` (`working` or the echo appears,
   then `working` detaches).

## Proof

Scorecard rows `create`, `echo`; UI rows `new-session`, `send-echo` with
`feedbackMs`/`replyMs`; session JSONL under `$PECAN_AGENT_DIR/sessions/`.

## Gotchas

- A running server embeds the web build it was compiled with; rebuild `web`
  then `cargo build` before a UI run.
