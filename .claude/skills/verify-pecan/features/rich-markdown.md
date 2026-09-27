# Rich markdown

Tables, code blocks, and Mermaid diagrams render legibly at phone width.

## Behaviors

- M1 a table renders.
- M2 a Mermaid diagram reaches `data-state="ready"`.
- M3 nothing makes the page scroll sideways.

## User entry points

- CLI: `session send <id> /rich` (raw markdown in `.turn.text`).
- UI: the thread.

## Drive

1. `session send <id> /rich --json` → text contains a mermaid fence.
2. UI smoke step `rich-markdown` → table visible, `mermaid[data-state=ready] svg`,
   `overflowPx == 0`, `diagramMs` recorded.

## Proof

Scorecard row `rich`; UI row `rich-markdown` and its screenshot.

## Gotchas

- The markdown renderer and Mermaid are lazy chunks; plain text shows until the renderer arrives (prefetched at idle).
