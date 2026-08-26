# Extension-dialog bridge proof (task 4)

Proof that Pi RPC extension dialogs — `select`, `confirm`, `input`, `editor` —
appear in Pecan and resolve back through it, plus the `pi --no-extensions`
baseline check. Task 2 supplied the Rust bridge (`record_pending_ask` /
`/asks` / `/respond` → `extension_ui_response` in `crates/pecan/src/server/`),
task 3 supplied the web host (`web/src/components/extension-host.tsx`). This
task only adds fixtures, a script, and this doc — no production code changed.

## Files added

| File | Role |
| --- | --- |
| `crates/pecan/tests/dialog_fixture.rs` | Automated end-to-end test: a fake `pi` binary speaks the RPC JSONL protocol to a real spawned `pecan serve`, and the test asserts the dialog requests appear (`GET /asks`) and resolve back to the worker on the wire (`extension_ui_response` frames recorded by the fake worker + registry drained). |
| `scripts/dialog-fixture-extension.ts` | Real Pi extension registering `/fixture-dialogs`, which emits all four blocking dialogs plus a cancelled `select` and records the resolved values. Never calls the model, so the script proof is offline and deterministic. |
| `scripts/prove-dialog-bridge.sh` | Runs real `pi --mode rpc --no-session --no-extensions --extension <fixture>`, answers each `extension_ui_request` with the exact frames Pecan's `/respond` builds (id echoed), and verifies the handler received every answer. Also emits a `notify` frame that the answer path must ignore. |

## Proof 1 — automated, through the real Pecan server

`cargo test -p pecan --test dialog_fixture` spawns the actual `pecan`
binary (`CARGO_BIN_EXE_pecan`) in a sandbox (`PECAN_AGENT_DIR` temp tree,
fake `pi` first on `PATH`), then drives the HTTP API:

1. `POST /api/session/new` — the fake worker answers `get_state` with
   `sessionId: fixture-session-1`, so new-session succeeds.
2. `POST /api/session/{id}/message` — the fake worker replies by emitting the
   four documented `extension_ui_request` frames (exact payloads from
   pi's `docs/rpc.md` "Extension UI Protocol").
3. `GET /api/session/{id}/asks` — asserts all four appear with the exact
   request ids and preserved renderable fields (`options`, `message`,
   `placeholder`, `prefill`).
4. `POST /api/session/{id}/respond` ×4 — the fake worker records every
   `extension_ui_response` frame Pecan writes to its stdin; the test asserts
   each frame echoes the exact request id with `value` / `confirmed`.
5. `GET /api/session/{id}/asks` — asserts the registry is empty again.

Pecan's worker reader, event forwarding, ask registry, `/asks`, `/respond`
and `send_raw` are the production code paths; only the `pi` executable is
simulated.

```text
$ cargo test -p pecan --test dialog_fixture
running 1 test
test extension_dialogs_appear_in_pecan_and_resolve_back_to_pi ... ok
test result: ok. 1 passed; 0 failed; finished in 0.72s
```

## Proof 2 — real pi, real extension

```text
$ scripts/prove-dialog-bridge.sh
   <- pi emitted extension_ui_request [select]   id=d5f3bc76-...
   -> sent extension_ui_response                 id=d5f3bc76-... {"type":"extension_ui_response","id":"...","value":"Allow"}
   <- pi emitted extension_ui_request [confirm]  ...
   -> sent extension_ui_response                 ... {"confirmed":true}
   <- pi emitted extension_ui_request [input]    ...
   -> sent extension_ui_response                 ... {"value":"typed answer"}
   <- pi emitted extension_ui_request [editor]   ...
   -> sent extension_ui_response                 ... {"value":"Edited\nLine 3"}
   <- pi emitted extension_ui_request [select]   ...        # second select: cancel path
   -> sent extension_ui_response                 ... {"cancelled":true}
   <- pi emitted extension_ui_request [notify]   ...        # fire-and-forget: correctly not answered
   verify  select   -> "Allow" (expected "Allow")
   verify  confirm  -> true (expected true)
   verify  input    -> "typed answer" (expected "typed answer")
   verify  editor   -> "Edited\nLine 3" (expected "Edited\nLine 3")
   verify  cancelled -> key absent (resolved undefined) (expected undefined)
== PROOF PASSED ==
```

This validates the wire contract the Rust bridge implements: blocking dialogs
resolve on stdin with the matching `id`; cancellation is `cancelled: true`;
fire-and-forget frames are ignored; a cancelled dialog resolves to
`undefined` on the extension side (which is what Pecan's UI shows as "cancel
did not change anything").

## Pi `--no-extensions` baseline

Checked live against pi 0.84.3 (default provider `openai-codex/gpt-5.6-sol`):

```text
$ pi --mode rpc --no-session --no-extensions   # then: get_state, prompt "Reply with the single word: OK"
get_state           -> sessionId: 01a03fbf-... | model: openai-codex/gpt-5.6-sol
agent_settled       -> turn completed under --no-extensions
```

- `get_state` answers normally; a prompt runs a full turn to `agent_settled`.
- Explicit `--extension <path>` still loads under `--no-extensions`
  (verified by Proof 2, which passes with the flag present), so a per-worker
  "no discovery, explicit extensions only" configuration is viable.
- Pecan's current `worker.rs` does not pass `--no-extensions` — discovery
  stays on; the baseline check covers the flag for the planned adapter work.

### Blocker report

**No hard blocker found** for either the dialog bridge or the
`--no-extensions` baseline. Findings worth knowing:

1. **Extension-provided content disappears under `--no-extensions`** (by
   design): the subagent-activity widget (`pi-subagents/activity/v1`), the
   `ask_user` tool (`pi-askuser`), tasks widgets (`pi-tasks`), etc. Pecan's
   web host renders these as empty/absent rather than breaking. If Pecan ever
   spawns workers with `--no-extensions`, those features need explicit
   `--extension` loads (the intended adapter design).
2. **`recordedAtMs` is second-granularity** (`Timestamp::as_second() * 1000`),
   so the `/asks` sort order across simultaneous dialogs is non-deterministic.
   Harmless today (the web host renders all pending asks and keys by id); left
   untouched per the "no production changes" scope.
3. **Pre-existing lint debt**: `cargo lint` is red at the committed baseline
   under the locally installed Homebrew clippy 1.97.1 (215–222 errors in
   `pecan`, 13–14 in `pecan-core`, mostly `disallowed-methods` on
   `unwrap`/`expect` and `map_err(|_| ...)`). The repo pins rust-toolchain
   1.91.0 via rust-toolchain.toml, which rustup would use; Homebrew's rust ignores
   it. None of these are introduced by this task; its test follows the
   existing in-repo test style (unwrap/expect allowed for tests via
   `clippy.toml`, which the CLI `disallowed-methods` entry still flags).

## Proof 3 — real pi + real pi-askuser (RPC fallback semantics)

The generic bridge is exercised against the REAL local pi-askuser package
(`~/.pi/agent/git/github.com/Yeshwanthyk/pi-askuser/index.ts`) instead of the
synthetic fixture. A deterministic script provider (`scripts/askuser-fixture-provider.ts`)
plays the model's role — `pi --mode rpc --no-session --no-extensions
--extension <pi-askuser> --extension <provider> --provider pecan-fixture
--model fixture-v1` — and the driver
(`scripts/prove-askuser-bridge.sh`) answers every `extension_ui_request`
frame with the exact frames pecan's `/respond` builds. Fully offline: the
provider never touches a network and the run is deterministic.

`--no-extensions` proves the adapter design from the baseline check: with
package discovery off, the two explicit `--extension` loads are sufficient
for `ask_user` to work — the same wiring `PECAN_PI_EXTENSIONS` gives the
pecan worker (`crates/pecan/src/server/worker.rs`, covered by Proof 1's argv
assertion).

Covered semantics, all asserted:

| Semantics | Scripted proof |
| --- | --- |
| Exact generated select values | `Pick a mode` emits `["Allow\u20630","Block","Allow\u20632","✏️ Other…"]`; answering the tagged `Allow\u20632` resolves to option 3 (`user selected option 3: Allow`). Ambiguous bare labels would not resolve. |
| Sequential questions | One batched call asks its four questions one dialog at a time (select → select → select×3 toggles → select → input), then a second scripted call runs after the first tool result. |
| Cancel | `cancelled: true` on `Second confirm` resolves the dialog to `undefined`; the interaction returns `dismissed` with the earlier answer preserved and the unanswered required question marked `not answered (required)`. |
| Optional skip | The optional question's dialog includes the `⏭ Skip (optional)` row; choosing it records `skippedOptionalQuestionIds: ["scope"]`. |
| Multi-select | `[ ]`/`[x]` toggles plus `✓ Done` commit `selections` with one-based indexes (`Search` 1, `Ship` 3). |
| Custom answer | `✏️ Other…` opens the `input` dialog; the value becomes a `wasCustom: true` answer. |
| Fire-and-forget | The shared `context` arrives first as a `notify` frame and is never answered. |

The provider additionally appends each tool result's exact model-facing text
to a report file; the driver checks both the live `tool_execution_end`
`details` and the report, so event-level and model-facing data agree.

```text
$ scripts/prove-askuser-bridge.sh
   <- pi emitted [notify] Shared: the user already reviewed the plan #1
   <- pi emitted [select] Pick a mode #2
   -> sent extension_ui_response        value="Allow⁣2"
   ...
   <- pi emitted [select] Second confirm #10
   -> sent extension_ui_response        cancelled=true
   verify  call 1 (completed): tagged duplicate -> option 3 Allow; skip -> [scope]
   verify  call 2 (dismissed): cancel mid-batch keeps the collected answer
   verify  report: model-facing text matches the bridge-delivered results
== PROOF PASSED ==
```

```
## Proof 4 — real pi + real pi-subagents (one Pi child: spawn → running → settle)

Task 7: the smallest end-to-end pi-subagents slice on top of the same
generic bridge. The real local pi-subagents package is loaded explicitly
(`~/.pi/agent/git/github.com/Yeshwanthyk/pi-subagents/extensions/subagents/index.ts`)
and one Pi child is spawned, observed running, and observed settled. The
activity-rail extension is intentionally NOT loaded — the
`pi-subagents/activity/v1` widget frames the proof observes come from the
subagents extension itself (the rail only renders activity; set
`PECAN_ACTIVITY_RAIL_EXTENSION` to opt it in). No production code changed:
the bridge, the `PECAN_PI_EXTENSIONS` env → per-worker `--extension`
mapping (`crates/pecan/src/server/worker.rs`), and the web host's widget
parsing (`web/src/events.ts`) already exist. This task adds only fixtures, a
script, a wiring test, and this doc.

| File | Role |
| --- | --- |
| `crates/pecan/tests/subagents_fixture.rs` | Automated wiring test: with `PECAN_PI_EXTENSIONS` set to the pi-subagents entry point, a real spawned `pecan serve` must pass it to the spawned worker as exactly one `--extension <subagents>` flag (fake `pi` dumps its argv); the worker unit tests cover the opt-in rail riding along in order. |
| `scripts/subagents-fixture-provider.ts` | Scripted parent model (`pecan-fixture/fixture-v1`): turn 1 issues `subagent_spawn` (harness `pi`, one child, explicit `fixture-child-v1` model), turn 2 issues `subagent_wait` on the spawned id, then it settles. Appends the observed spawn/wait result text to the report file. |
| `scripts/prove-subagents-bridge.sh` | Real-pi proof driver: stages a temp project + agent dir, runs a local offline OpenAI-completions mock as the child's model, spawns real `pi --mode rpc --no-extensions` with the explicit loads, and verifies the full one-child lifecycle. |

The parent model is the scripted provider; the child is a genuine
pi-subagents in-process pi SDK session (`createAgentSession`, the pi
backend). Because each child builds a **fresh `ModelRuntime`** that only
sees the agent dir's `auth.json` + `models.json` (never in-memory extension
registrations), the proof stages a scratch agent dir (`PI_CODING_AGENT_DIR`)
declaring `pecan-fixture` as an openai-completions provider whose baseUrl is
the local mock — exactly how real providers reach children. The fixture
provider's own baseUrl points at the same mock (via `PECAN_FIXTURE_PORT`),
because the model object the child inherits carries it. The mock replies
with the scripted text after a fixed delay, so the running widget is always
observable. Nothing touches the network or the real `~/.pi/agent`.

| Slice step | Proof |
| --- | --- |
| Spawned | `tool_execution_end` of `subagent_spawn` returns `details.id` (`sa-N`), harness `pi`, model `pecan-fixture/fixture-child-v1`, and the parent context gets `Spawned subagent sa-1 …`. |
| Observed running | The browser activity widget (`extension_ui_request`/`setWidget`, widgetKey `pi-subagents/activity/v1`) carries the child with `status: "running"` — the exact frames pecan's web host parses, so a pecan websocket would render the same strip rows. |
| Settled | `subagent_wait` (which blocks on the running child) returns `## sa-1 "Fixture child" finished` plus the child's final text; the widget's `terminal` snapshot flips to `{status: "done", output: …}`; the parent then reaches `agent_settled`. |
| Real child | The child's persisted session jsonl (under the temp agent dir) contains the prompt it received and the reply it produced, and the fixture report file agrees with the wire-observed tool results. |

```text
$ scripts/prove-subagents-bridge.sh
== pecan pi-subagents bridge proof (real pi + real pi-subagents) ==
   pi:               /Users/yesh/.nvm/versions/node/v22.22.2/bin/pi (0.84.3)
   pi-subagents:     …/pi-subagents/extensions/subagents/index.ts
   fixture model:    …/scripts/subagents-fixture-provider.ts (pecan-fixture/fixture-v1 + local offline mock)
   mock model:       http://127.0.0.1:57461/v1 (offline, scripted "subagent done")
   -> sent prompt (parent: subagent_spawn -> subagent_wait -> settle)
   <- subagent_spawn -> sa-1 [pi] Fixture child
      Spawned subagent sa-1 "Fixture child" (pi: pecan-fixture/fixture-child-v1, …/pecan-sa-cwd-…).
   <- subagent_wait returned (child settled)
   verify  spawned:  sa-1 Fixture child (pi)
   verify  running:  pi-subagents/activity/v1 widget listed sa-1 with status "running"
   verify  settled:  wait returned {status: done, output: "subagent done"}
   verify  parent:   agent_settled after the one-child run
   verify  report:   provider-observed spawn/wait results agree with the wire
   verify  child:    session …jsonl persisted prompt + "subagent done"
== PROOF PASSED ==
```

The `PECAN_PI_EXTENSIONS` value for pecan operators is the pi-subagents
subagents entry point alone (`…/pi-subagents/extensions/subagents/index.ts`);
`crates/pecan/tests/subagents_fixture.rs` asserts that mapping reaches the
spawned worker's argv. The activity rail is not needed for the widget and is
left opt-in: the proof script can append
`…/pi-subagents/extensions/activity-rail/index.ts` via `PECAN_ACTIVITY_RAIL_EXTENSION`,
and operators composing their own environment value may append it to
`PECAN_PI_EXTENSIONS` (the worker unit tests pin both shapes).

## Run everything

```bash
cargo test -p pecan --test dialog_fixture            # automated proof (fake pi)
cargo test -p pecan --test subagents_fixture         # pi-subagents wiring (fake pi)
cargo test -p pecan worker::tests                    # PECAN_PI_EXTENSIONS wiring unit tests
scripts/prove-dialog-bridge.sh                       # real-pi fixture-extension proof
scripts/prove-askuser-bridge.sh                      # real pi + real pi-askuser proof
scripts/prove-subagents-bridge.sh                    # real pi + real pi-subagents proof
```
