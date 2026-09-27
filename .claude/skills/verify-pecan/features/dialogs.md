# Extension dialogs

An extension asks the user to confirm; the turn waits and resumes on the answer.

## Behaviors

- D1 opening a dialog pauses the turn (`waiting-for-user`).
- D2 answering resumes and settles the turn.

## User entry points

- CLI: `pecan session asks <id>`, `pecan session respond <id> <askId> --confirm|--deny|--cancel`.
- UI: the dialog card above the composer.

## Drive

1. `session send <id> /confirm --json` → `.turn.outcome == "waiting-for-user"`,
   `.turn.dialog.method == "confirm"`.
2. `session respond <id> <dialog.id> --confirm --json` → settled, text contains
   `confirmed`.

## Proof

Scorecard rows `confirm-opens`, `confirm-answered`.

## Gotchas

- Only select/confirm/input/editor block; notify/setStatus never do.
