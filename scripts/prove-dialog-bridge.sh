#!/usr/bin/env bash
#
# Real-pi extension-dialog proof for the pecan extension bridge.
#
# Spawns the real `pi` binary in RPC mode -- with `--no-extensions` (the
# baseline flag; the fixture extension is loaded explicitly via `--extension`,
# which `--no-extensions` still honors) -- and drives the documented extension
# UI sub-protocol exactly the way Pecan does:
#
#   1. the `/fixture-dialogs` command handler emits four blocking
#      `extension_ui_request` frames (select, confirm, input, editor) on stdout
#   2. the driver answers each on stdin with an `extension_ui_response`
#      echoing the exact request `id` -- the same frames Pecan writes from its
#      `/respond` endpoint
#   3. the handler records the resolved values, including `undefined` for the
#      cancelled dialog, and the driver verifies them
#
# The command makes no model calls, so this proof is fully offline and
# deterministic: it validates the wire contract, not the LLM.
#
# Usage:
#   scripts/prove-dialog-bridge.sh [--results /tmp/out.json]
#
# Exit status 0 = proof passed; 1 = any step failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

RESULTS="${PECAN_DIALOG_FIXTURE_OUT:-/tmp/pecan-dialog-fixture-results.json}"
if [[ "${1:-}" == "--results" ]]; then
  RESULTS="$2"
fi
rm -f "$RESULTS"
EXTENSION="$REPO_ROOT/scripts/dialog-fixture-extension.ts"

if ! command -v pi >/dev/null 2>&1; then
  echo "FATAL: 'pi' not found on PATH" >&2
  exit 1
fi
echo "== pecan extension-dialog bridge proof =="
echo "   pi:              $(command -v pi) ($(pi --version))"
echo "   extension:       $EXTENSION"
echo "   results file:    $RESULTS"

# The driver is a small Node client so JSONL handling is exact (split on \n
# only, as the RPC protocol demands).
node - "$RESULTS" "$EXTENSION" <<'EOF' && echo "== PROOF PASSED =="
const { spawn } = require("node:child_process");
const { readFileSync, existsSync } = require("node:fs");
const { join } = require("node:path");

const [resultsPath, extensionPath] = process.argv.slice(2);
const print = (line) => process.stdout.write(line + "\n");

const pi = spawn("pi", [
  "--mode", "rpc",
  "--no-session",
  "--no-extensions", // baseline: no extension *discovery*; -e still loads
  "--extension", extensionPath,
], { stdio: ["pipe", "pipe", "pipe"] });

let stderr = "";
pi.stderr.on("data", (c) => { stderr += c.toString(); });

const requests = [];
let done = false;
let promptSent = false;
let selectCount = 0;
let buf = "";

function send(obj) {
  pi.stdin.write(JSON.stringify(obj) + "\n");
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

    if (event.type === "extension_ui_request") {
      requests.push(event);
      print(`   <- pi emitted extension_ui_request [${event.method}] id=${event.id}`);
      let frame;
      if (event.method === "select") {
        selectCount += 1;
        if (selectCount === 1) {
          // First select: answer with the first option's exact wire value.
          frame = { type: "extension_ui_response", id: event.id, value: event.options[0] };
        } else {
          // Second select: cancel (Pecan's cancel path: cancelled: true).
          frame = { type: "extension_ui_response", id: event.id, cancelled: true };
        }
      } else if (event.method === "confirm") {
        frame = { type: "extension_ui_response", id: event.id, confirmed: true };
      } else if (event.method === "input") {
        frame = { type: "extension_ui_response", id: event.id, value: "typed answer" };
      } else if (event.method === "editor") {
        frame = { type: "extension_ui_response", id: event.id, value: "Edited\nLine 3" };
      } else {
        return; // fire-and-forget frames: nothing to answer
      }
      if (frame) {
        send(frame);
        print(`   -> sent extension_ui_response           id=${event.id} ${JSON.stringify(frame)}`);
      }
      return;
    }
  }
});

function fail(message) {
  print("== PROOF FAILED ==");
  print(message);
  if (stderr.trim()) print("pi stderr: " + stderr.trim().split("\n")[0]);
  pi.kill("SIGKILL");
  process.exit(1);
}

// Kick off the fixture command once the RPC session is ready.
setTimeout(() => {
  if (promptSent) return;
  promptSent = true;
  send({ type: "prompt", message: `/fixture-dialogs ${resultsPath}` });
}, 2000);

// Wait for the handler to write its results file, then verify.
const expected = {
  select: "Allow",
  confirm: true,
  input: "typed answer",
  editor: "Edited\nLine 3",
};
const deadline = Date.now() + 30000;
const poll = setInterval(() => {
  if (!existsSync(resultsPath)) {
    if (Date.now() > deadline) fail("timed out waiting for the extension results file");
    return;
  }
  const results = JSON.parse(readFileSync(resultsPath, "utf8"));
  for (const [key, value] of Object.entries(expected)) {
    print(`   verify  ${key.padEnd(8)} -> ${JSON.stringify(results[key])} (expected ${JSON.stringify(value)})`);
    if (results[key] !== value) fail(`unexpected resolved value for ${key}`);
  }
  const cancelledKeyAbsent = !("cancelled" in results);
  print(`   verify  cancelled -> ${cancelledKeyAbsent ? "key absent (resolved undefined)" : JSON.stringify(results.cancelled)} (expected undefined)`);
  if (!cancelledKeyAbsent) fail("cancelled dialog should resolve to undefined");

  // Fire-and-forget frames (notify/status/widget) must be ignored by the
  // answer path; only the blocking dialogs matter for the sequence.
  const answeredSelects = requests.filter((r) => r.method === "select").length;
  if (answeredSelects !== 2) fail(`expected two select dialogs, saw ${answeredSelects}`);
  const blocking = new Set(["select", "confirm", "input", "editor"]);
  const methods = requests.filter((r) => blocking.has(r.method)).map((r) => r.method).sort().join(",");
  const expectedMethods = "confirm,editor,input,select,select";
  if (methods !== expectedMethods) fail(`unexpected dialog sequence: ${methods}`);

  clearInterval(poll);
  done = true;
  pi.kill();
}, 100);

pi.on("exit", (code) => {
  if (done) {
    // We terminated pi ourselves after verifying; that is a pass.
    print("   pi terminated by the driver after verification");
    process.exit(0);
  }
  if (code !== 0 && code !== null) fail(`pi exited with code ${code}`);
});
EOF