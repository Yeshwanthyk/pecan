#!/usr/bin/env bash
#
# Real pi + real pi-askuser bridge proof.
#
# Spawns the real `pi` binary in RPC mode with `--no-extensions` (no package
# discovery) plus two explicit `--extension` loads: the REAL local pi-askuser
# package and a deterministic script provider that plays the model's role.
# Nothing else from the user's settings is loaded, and no network is touched:
# the fixture provider only ever emits scripted `ask_user` tool calls.
#
# The driver then speaks the exact wire contract pecan's bridge implements —
# `extension_ui_request` frames are answered with the same
# `extension_ui_response` shapes pecan's `/respond` endpoint builds — and
# verifies pi-askuser's RPC fallback semantics end to end:
#
#   1. exact generated select values, including the U+2063-tagged duplicate
#      labels and the ✏️ Other… / ⏭ Skip (optional) / ✓ Done rows
#   2. sequential questions (a batch asks one dialog at a time, then a second
#      scripted call runs after the first tool result)
#   3. cancel (cancelled: true → the extension resolves `undefined` →
#      dismissed with the partial answer preserved)
#   4. optional skip (the skip row records skippedOptionalQuestionIds)
#   5. multi-select (checkbox toggles + Done commit indexes) and a custom
#      write-your-own answer through the input dialog
#   6. the fire-and-forget notify frame (shared context) is emitted but never
#      answered
#
# Usage:
#   scripts/prove-askuser-bridge.sh [--results /tmp/pecan-askuser-proof-results.jsonl]
#
# The pi-askuser entry is `$PECAN_ASKUSER_EXTENSION` or the default local
# package path. Exit status 0 = proof passed; 1 = any step failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

RESULTS="${PECAN_ASKUSER_FIXTURE_OUT:-/tmp/pecan-askuser-proof-results.jsonl}"
if [[ "${1:-}" == "--results" ]]; then
  RESULTS="$2"
fi
: > "$RESULTS"

PROVIDER="$REPO_ROOT/scripts/askuser-fixture-provider.ts"
ASKUSER="${PECAN_ASKUSER_EXTENSION:-$HOME/.pi/agent/git/github.com/Yeshwanthyk/pi-askuser/index.ts}"

if ! command -v pi >/dev/null 2>&1; then
  echo "FATAL: 'pi' not found on PATH" >&2
  exit 1
fi
if [[ ! -f "$ASKUSER" ]]; then
  echo "FATAL: pi-askuser extension not found at $ASKUSER" >&2
  echo "       set PECAN_ASKUSER_EXTENSION to the real package's index.ts" >&2
  exit 1
fi

echo "== pecan pi-askuser bridge proof (real pi + real pi-askuser) =="
echo "   pi:             $(command -v pi) ($(pi --version))"
echo "   pi-askuser:     $ASKUSER"
echo "   fixture model:  $PROVIDER (pecan-fixture/fixture-v1)"
echo "   results file:   $RESULTS"

node - "$RESULTS" "$ASKUSER" "$PROVIDER" <<'EOF' && echo "== PROOF PASSED =="
const { spawn } = require("node:child_process");
const { readFileSync, existsSync } = require("node:fs");

const [resultsPath, askuserPath, providerPath] = process.argv.slice(2);
const print = (line) => process.stdout.write(line + "\n");
let failed = false;
const fail = (message) => {
  if (failed) return;
  failed = true;
  print("== PROOF FAILED ==");
  print(message);
  if (stderr.trim()) print("pi stderr tail: " + stderr.trim().split("\n").slice(-4).join(" | "));
  try { pi.kill("SIGKILL"); } catch {}
  process.exit(1);
};

const pi = spawn("pi", [
  "--mode", "rpc", "--no-session",
  "--no-extensions",            // no discovery: only the two explicit loads
  "--extension", askuserPath,
  "--extension", providerPath,
  "--provider", "pecan-fixture",
  "--model", "fixture-v1",
], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, ASKUSER_FIXTURE_OUT: resultsPath } });

let stderr = "";
pi.stderr.on("data", (c) => { stderr += c.toString(); });

const send = (obj) => pi.stdin.write(JSON.stringify(obj) + "\n");

const TAG = "\u2063"; // U+2063 WORD JOINER: pi-askuser's ambiguity tag
const OTHER = "✏️ Other…";
const SKIP = "⏭ Skip (optional)";
const DONE = "✓ Done";

// Scripted expectations, in exact arrival order. `respond` describes the
// extension_ui_response frames pecan's /respond endpoint would build.
const DIALOGS = [
  { method: "notify", message: "Shared: the user already reviewed the plan", notifyType: "info" },
  { method: "select", title: "Pick a mode", options: ["Allow" + TAG + "0", "Block", "Allow" + TAG + "2", OTHER], respond: (e) => ({ value: e.options[2] }) },
  { method: "select", title: "Where should it apply?", options: ["Here", "There", OTHER, SKIP], respond: (e) => ({ value: e.options[3] }) },
  { method: "select", title: "Which features? (select all that apply)", options: ["[ ] Search", "[ ] Diff", "[ ] Ship", OTHER, DONE], respond: (e) => ({ value: e.options[0] }) },
  { method: "select", title: "Which features? (select all that apply)", options: ["[x] Search", "[ ] Diff", "[ ] Ship", OTHER, DONE], respond: (e) => ({ value: e.options[2] }) },
  { method: "select", title: "Which features? (select all that apply)", options: ["[x] Search", "[ ] Diff", "[x] Ship", OTHER, DONE], respond: (e) => ({ value: e.options[4] }) },
  { method: "select", title: "Name the bot", options: ["Pecan", "Walnut", OTHER], respond: (e) => ({ value: e.options[2] }) },
  { method: "input", title: "Name the bot", placeholder: "Type your answer", respond: () => ({ value: "Copper Cod" }) },
  { method: "select", title: "First confirm", options: ["Yes", "No", OTHER], respond: (e) => ({ value: e.options[0] }) },
  { method: "select", title: "Second confirm", options: ["Okay", "Nope", OTHER], respond: () => ({ cancelled: true }) },
];

const toolResults = [];
let dialogs = 0;
let promptSent = false;
let settled = false;
let buf = "";

function handle(event) {
  if (event.type === "agent_settled") { settled = true; return; }
  if (event.type === "tool_execution_end" && event.toolName === "ask_user") {
    toolResults.push({
      text: (event.result?.content ?? [])
        .filter((b) => b.type === "text" && b.text)
        .map((b) => b.text)
        .join("\n"),
      details: event.result?.details ?? {},
    });
    return;
  }
  if (event.type !== "extension_ui_request") return;
  const expected = DIALOGS[dialogs];
  if (!expected) return fail(`unexpected ${event.method} frame (${dialogs} dialogs already handled)`);
  const ok =
    event.method === expected.method &&
    (expected.title === undefined || event.title === expected.title) &&
    (expected.options === undefined || JSON.stringify(event.options) === JSON.stringify(expected.options)) &&
    (expected.placeholder === undefined || event.placeholder === expected.placeholder) &&
    (expected.message === undefined || event.message === expected.message) &&
    (expected.notifyType === undefined || event.notifyType === expected.notifyType);
  if (!ok) {
    return fail(
      `dialog ${dialogs + 1} mismatch\n  expected: ${JSON.stringify(expected)}\n  actual:   ${JSON.stringify(event)}`,
    );
  }
  print(`   <- pi emitted [${event.method}] ${expected.title ?? expected.message ?? ""} #${dialogs + 1}`);
  if (event.method === "notify") {
    dialogs += 1;
    return; // fire-and-forget: nothing to answer
  }
  const frame = { type: "extension_ui_response", id: event.id, ...expected.respond(event) };
  dialogs += 1;
  send(frame);
  print(`   -> sent extension_ui_response        ${frame.value !== undefined ? "value=" + JSON.stringify(frame.value) : frame.cancelled ? "cancelled=true" : ""}`);
}

pi.stdout.on("data", (chunk) => {
  buf += chunk.toString();
  let idx;
  while ((idx = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, idx);
    buf = buf.slice(idx + 1);
    if (!line.trim()) continue;
    let event;
    try { event = JSON.parse(line); } catch { continue; }
    handle(event);
  }
});

// Kick off the run once pi is up.
setTimeout(() => {
  if (promptSent) return;
  promptSent = true;
  send({ type: "prompt", message: "Drive the ask_user proof" });
  print("   -> sent prompt (turn 1: scripted 4-question batch; turn 2: cancel)");
}, 2000);

const deadline = Date.now() + 60000;
const poll = setInterval(() => {
  if (failed) return clearInterval(poll);
  if (!settled) {
    if (Date.now() > deadline) fail("timed out waiting for agent_settled");
    return;
  }
  if (dialogs !== DIALOGS.length) return fail(`expected ${DIALOGS.length} dialog frames, saw ${dialogs}`);
  if (toolResults.length !== 2) return fail(`expected 2 ask_user tool results, saw ${toolResults.length}`);

  // 1) Tool-result 1: completed, exact answers and skip.
  const one = toolResults[0];
  if (one.details.status !== "completed") return fail(`call 1 status: ${one.details.status}`);
  if (one.details.cancelled !== false) return fail("call 1 must not be cancelled");
  const expectedText1 = [
    "[mode] Pick a mode: user selected option 3: Allow",
    "[scope] Where should it apply?: skipped (optional)",
    "[features] Which features?: user selected option 1: Search; user selected option 3: Ship",
    "[nickname] Name the bot: user wrote: Copper Cod",
  ].join("\n");
  if (one.text !== expectedText1) return fail(`call 1 text mismatch:\n${JSON.stringify(one.text)}`);
  if (JSON.stringify(one.details.skippedOptionalQuestionIds) !== JSON.stringify(["scope"])) {
    return fail(`call 1 skipped: ${JSON.stringify(one.details.skippedOptionalQuestionIds)}`);
  }
  const answers1 = JSON.stringify(one.details.answers);
  if (!answers1.includes('"id":"mode"') || !answers1.includes('"answer":"Allow"') || !answers1.includes('"index":3')) {
    return fail(`call 1 mode answer missing: ${answers1}`);
  }
  if (!answers1.includes('"multiSelect":true') || !answers1.includes('"answer":"Search"') || !answers1.includes('"answer":"Ship"')) {
    return fail(`call 1 multi-select answer missing: ${answers1}`);
  }
  if (!answers1.includes('"answer":"Copper Cod"') || !answers1.includes('"wasCustom":true')) {
    return fail(`call 1 custom answer missing: ${answers1}`);
  }
  print("   verify  call 1 (completed): tagged duplicate -> option 3 Allow; skip -> [scope]");

  // 2) Tool-result 2: dismissed on cancel, partial answer preserved.
  const two = toolResults[1];
  if (two.details.status !== "dismissed") return fail(`call 2 status: ${two.details.status}`);
  if (two.details.cancelled !== false) return fail("call 2 must not be cancelled");
  if (!two.text.startsWith("User dismissed the question UI.")) return fail(`call 2 must say dismissed: ${two.text}`);
  if (!two.text.includes("[second-confirm] Second confirm: not answered (required)")) {
    return fail(`call 2 must mark the unanswered required question: ${two.text}`);
  }
  const answers2 = JSON.stringify(two.details.answers);
  if (!answers2.includes('"id":"first-confirm"') || !answers2.includes('"answer":"Yes"')) {
    return fail(`call 2 must preserve the partial answer: ${answers2}`);
  }
  print("   verify  call 2 (dismissed): cancel mid-batch keeps the collected answer");

  // 3) The model-facing report file matches the bridge-delivered result text.
  if (!existsSync(resultsPath)) return fail("fixture provider never wrote the results file");
  const lines = readFileSync(resultsPath, "utf8").trim().split("\n").filter(Boolean);
  if (lines.length !== 2) return fail(`expected 2 report lines, saw ${lines.length}`);
  const report = lines.map((line) => JSON.parse(line));
  if (report[0].call !== 1 || report[1].call !== 2) return fail(`report call ids: ${JSON.stringify(report)}`);
  if (report[0].text !== one.text) return fail("report call 1 differs from the bridge result text");
  if (report[1].text !== two.text) return fail("report call 2 differs from the bridge result text");
  print("   verify  report: model-facing text matches the bridge-delivered results");

  clearInterval(poll);
  print("   pi settled after the scripted two-call run");
  pi.kill();
  process.exit(0);
}, 100);

pi.on("exit", (code) => {
  if (failed) return;
  if (code !== 0 && code !== null) fail(`pi exited with code ${code}`);
});
EOF