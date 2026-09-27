# Tools and thinking

Tool calls and model thinking show as one compact activity row per tool loop.

## Behaviors

- T1 a tool call runs and its result feeds the reply.
- T2 thinking text is shown as a single muted line.

## User entry points

- CLI: `pecan session send <id> /tool` and `/think <topic>`.
- UI: the same prompts through the composer.

## Drive

1. `session send <id> /tool --json` → `.turn.tools == ["bash"]`, text contains
   `fixture-tool-ok`.
2. `session send <id> "/think deeply" --json` → `.turn.text == "Thought about: deeply"`.

## Proof

Scorecard rows `tool`, `think`; `pecan thread <id>` lists the tool row.

## Gotchas

- `/tool` runs a real `bash` echo inside the scratch project.
