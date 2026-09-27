# Turn errors

A failed or stopped turn says so in the thread instead of silently vanishing.

## Behaviors

- E1 provider error text shows as an error row (`turn-error`).
- E2 an aborted turn shows a muted "Stopped" (`turn-stopped`).

## User entry points

- CLI: `session send <id> /error`, then `pecan thread <id>` (prints `ERROR`).
- UI: the thread.

## Drive

1. `session send <id> /error --json` → `.turn.error == "fixture provider error"`.
2. UI smoke step `turn-error` waits for `[data-testid=turn-error]` with that text.

## Proof

Scorecard row `error`; UI row `turn-error` and its screenshot.

## Gotchas

- Errors come from `stopReason:"error"` in the session file, so they survive reload.
