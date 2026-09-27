# Run control

While the agent is working the user can steer it, queue a follow-up, or stop it.

## Behaviors

- R1 a plain send while busy is rejected.
- R2 steer lands after the current assistant message.
- R3 queue runs after the current turn.
- R4 abort stops the worker.

## User entry points

- CLI: `session send <id> <text> --mode steer|queue`, `session abort <id>`.
- UI: composer while streaming; Esc or the `abort` button stops.

## Drive

1. `session send <id> /slow --no-wait`, then `session send <id> x` → non-zero exit.
2. `--mode steer` / `--mode queue` during `/slow` → settled echo of the new text.
3. `session abort <id> --json` → `.aborted == true`.

## Proof

Scorecard rows `busy-send-rejected`, `steer`, `queue`, `abort`, `state`.

## Gotchas

- Steer/queue take ~6s: Pi applies them after `/slow` finishes its message.
- Abort removes the worker, so do not wait for `agent_settled` after it.
