# Pecan feature map

Baseline: an isolated launch with the fixture model and one project added
(see ../SKILL.md, Launch and Doctor). Drive through the CLI first; the browser
smoke proves the phone UI for the same flows. Evidence is kept in the
`--keep` scratch dir.

| Recipe | User-visible behavior |
| --- | --- |
| [sessions.md](sessions.md) | Start a session, send, stream a reply, read it back |
| [tools-and-thinking.md](tools-and-thinking.md) | Tool calls and thinking render as compact activity |
| [dialogs.md](dialogs.md) | Extension confirm/select/input dialogs block and resume a turn |
| [run-control.md](run-control.md) | Busy rejection, steer, queue, abort |
| [turn-errors.md](turn-errors.md) | Provider errors and aborts are visible in the thread |
| [rich-markdown.md](rich-markdown.md) | Tables, code, and Mermaid render on a phone |
| [pairing.md](pairing.md) | Unpaired browsers are refused; one-time codes pair a phone; revoke signs it out |
| [remote.md](remote.md) | `pecan remote` reports and enables the Tailscale HTTPS URL without clobbering other ports |
| [push.md](push.md) | Finished turns and waiting dialogs push to subscribed devices that are not watching |
| [resilience.md](resilience.md) | Health, retry-safe sends, SSE replay after reconnect, worker death, attention |

## Not yet covered

These are kept features without a fixture-driven recipe yet: ship, diff,
tasks, workflows, subagent activity, title regeneration. Each needs a git
project or a real extension in the scratch agent dir; add a recipe before
claiming them verified.
