#!/usr/bin/env bash
#
# Real pi + real pi-subagents bridge proof: spawn -> running -> settle.
#
# The smallest end-to-end pi-subagents slice on top of pecan's generic Pi
# extension bridge. Spawns the real `pi` binary in RPC mode with
# `--no-extensions` (no package discovery) plus explicit `--extension`
# loads: the REAL local pi-subagents extension and a deterministic script
# provider that plays the parent model's role. The subagents extension
# itself publishes the `pi-subagents/activity/v1` widget frames used to
# OBSERVE a child as running (the activity-rail extension is NOT needed for
# the widget; set PECAN_ACTIVITY_RAIL_EXTENSION to opt it in). This is
# exactly the wiring `PECAN_PI_EXTENSIONS` gives pecan's worker
# (`crates/pecan/src/server/worker.rs`; see
# `crates/pecan/tests/subagents_fixture.rs` for the argv assertion).
#
# Fully offline and deterministic:
#   - the parent model is a scripted provider (`pecan-fixture/fixture-v1`)
#     that issues `subagent_spawn` (one Pi child) and then `subagent_wait`
#   - the child is a real in-process pi SDK session per pi-subagents; its
#     fresh model runtime reads the temp agent dir's `auth.json` +
#     `models.json` (as real providers do), where `pecan-fixture` points at
#     a local mock OpenAI-completions endpoint served by this driver
#   - the mock replies with the scripted text after a fixed 600ms delay, so
#     the activity widget visibly shows the child RUNNING before it settles
#   - nothing touches the network or the real ~/.pi agent dir (the proof
#     stages its own agent dir via `PI_CODING_AGENT_DIR`)
#
# The driver then proves the one-child lifecycle against the running parent
# RPC stream:
#   1. `subagent_spawn` tool result carries the child id (`sa-N`, harness
#      `pi`) into the parent context
#   2. a `setWidget` frame for `pi-subagents/activity/v1` lists the child
#      with status `running` (the same frames pecan's web host renders)
#   3. `subagent_wait` returns `## sa-N "…" finished` with the child's final
#      text; the widget terminal snapshot flips to `done`
#   4. the parent emits `agent_settled`
#   5. the child's persisted session jsonl (under the temp agent dir)
#      contains its prompt and the scripted reply, and the fixture report
#      file agrees with the RPC-observed tool results
#
# Usage:
#   scripts/prove-subagents-bridge.sh [--results /tmp/out.jsonl]
#
# Environment overrides:
#   PECAN_SUBAGENTS_EXTENSION          pi-subagents entry file
#   PECAN_ACTIVITY_RAIL_EXTENSION      optional rail entry (unset = not loaded)
#   SUBAGENTS_FIXTURE_OUT              report file
#
# Exit status 0 = proof passed; 1 = any step failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

RESULTS="${SUBAGENTS_FIXTURE_OUT:-/tmp/pecan-subagents-proof-results.jsonl}"
if [[ "${1:-}" == "--results" ]]; then
  RESULTS="$2"
fi
: > "$RESULTS"

PROVIDER="$REPO_ROOT/scripts/subagents-fixture-provider.ts"
PRIMARY="${PECAN_SUBAGENTS_EXTENSION:-$HOME/.pi/agent/git/github.com/Yeshwanthyk/pi-subagents/extensions/subagents/index.ts}"
# The activity rail only renders activity; the widget frames the proof needs
# come from the subagents extension, so the rail stays opt-in.
RAIL="${PECAN_ACTIVITY_RAIL_EXTENSION:-}"

if ! command -v pi >/dev/null 2>&1; then
  echo "FATAL: 'pi' not found on PATH" >&2
  exit 1
fi
if [[ ! -f "$PRIMARY" ]]; then
  echo "FATAL: pi-subagents extension not found at $PRIMARY" >&2
  echo "       set PECAN_SUBAGENTS_EXTENSION to the package's extensions/subagents/index.ts" >&2
  exit 1
fi
if [[ -n "$RAIL" && ! -f "$RAIL" ]]; then
  echo "FATAL: pi-subagents activity rail not found at $RAIL" >&2
  echo "       unset PECAN_ACTIVITY_RAIL_EXTENSION or point it at the package's extensions/activity-rail/index.ts" >&2
  exit 1
fi

echo "== pecan pi-subagents bridge proof (real pi + real pi-subagents) =="
echo "   pi:               $(command -v pi) ($(pi --version))"
echo "   pi-subagents:     $PRIMARY"
if [[ -n "$RAIL" ]]; then echo "   activity rail:    $RAIL"; fi
echo "   fixture model:    $PROVIDER (pecan-fixture/fixture-v1 + local offline mock)"
echo "   results file:     $RESULTS"

node - "$RESULTS" "$PRIMARY" "$RAIL" "$PROVIDER" <<'EOF' && echo "== PROOF PASSED =="
const { spawn } = require("node:child_process");
const http = require("node:http");
const { mkdirSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");

const [resultsPath, primaryPath, railPath, providerPath] = process.argv.slice(2);
const print = (line) => process.stdout.write(line + "\n");
let failed = false;

const CHILD_REPLY = "subagent done";
const CHILD_PROMPT = "Reply with exactly the text: subagent done";
const CHILD_TITLE = "Fixture child";
const WIDGET_KEY = "pi-subagents/activity/v1";

// --- Stage: temp project + agent dirs, seeded like a real provider setup. --
const CWD = mkdtempSync(join(tmpdir(), "pecan-sa-cwd-"));
const AGENT = mkdtempSync(join(tmpdir(), "pecan-sa-agent-"));
mkdirSync(join(AGENT, "sessions"), { recursive: true });
// The child (fresh in-process runtime) resolves provider auth from here.
writeFileSync(join(AGENT, "auth.json"), JSON.stringify({
  "pecan-fixture": { type: "api_key", key: "fixture-key" },
}));

// --- Offline mock: plays the child's model via a local OpenAI-completions
// endpoint. Answers every request with the scripted reply after a fixed
// delay so the activity widget shows the child RUNNING before settle. ------
const mock = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => { body += chunk.toString(); });
  req.on("end", () => {
    let parsed = {};
    try { parsed = JSON.parse(body); } catch {}
    if ((req.url ?? "").endsWith("/models")) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ object: "list", data: [{ id: "fixture-child-v1", object: "model" }] }));
      return;
    }
    const actualModel = parsed.model ?? "fixture-child-v1";
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    const id = `chatcmpl-pecan-fixture-${Date.now()}`;
    const chunk = (delta, finish_reason) =>
      `data: ${JSON.stringify({ id, object: "chat.completion.chunk", created: Date.now(), model: actualModel, choices: [{ index: 0, delta, finish_reason }] })}\n\n`;
    void (async () => {
      // Guarantees at least one widget frame with status "running".
      await new Promise((resolve) => setTimeout(resolve, 600));
      res.write(chunk({ role: "assistant", content: "" }, null));
      await new Promise((resolve) => setTimeout(resolve, 80));
      res.write(chunk({ content: CHILD_REPLY }, null));
      await new Promise((resolve) => setTimeout(resolve, 80));
      res.write(chunk({}, "stop"));
      res.write("data: [DONE]\n\n");
      res.end();
    })();
  });
});

/** Module-scope pi stderr accumulation (fail() may run before boot() sets it). */
let stderr = "";

function fail(message) {
  if (failed) return;
  failed = true;
  print("== PROOF FAILED ==");
  print(message);
  if (stderr.trim()) print("pi stderr tail: " + stderr.trim().split("\n").slice(-4).join(" | "));
  try { pi.kill("SIGKILL"); } catch {}
  process.exit(1);
}

mock.listen(0, "127.0.0.1", () => {
  const port = mock.address().port;
  const baseUrl = `http://127.0.0.1:${port}/v1`;
  // The child's fresh runtime reads THIS provider def (as real providers do).
  writeFileSync(join(AGENT, "models.json"), JSON.stringify({
    providers: {
      "pecan-fixture": {
        name: "Pecan Fixture",
        baseUrl,
        api: "openai-completions",
        apiKey: "fixture-key",
        compat: {
          supportsStore: false,
          supportsDeveloperRole: false,
          supportsReasoningEffort: false,
          supportsUsageInStreaming: false,
          maxTokensField: "max_tokens",
          supportsStrictMode: false,
          supportsLongCacheRetention: false,
        },
        models: [
          { id: "fixture-v1", name: "Fixture Model", reasoning: false, input: ["text"], contextWindow: 64000, maxTokens: 4096 },
          { id: "fixture-child-v1", name: "Fixture Child", reasoning: false, input: ["text"], contextWindow: 64000, maxTokens: 4096 },
        ],
      },
    },
  }));
  print(`   mock model:       ${baseUrl} (offline, scripted ${JSON.stringify(CHILD_REPLY)})`);
  print(`   staged agent dir: ${AGENT}`);
  boot(port, baseUrl);
});

function boot(port, baseUrl) {
  const extensionArgs = ["--extension", primaryPath, "--extension", providerPath];
  // The activity rail only renders activity; the widget frames the proof
  // observes come from the subagents extension, so the rail is opt-in.
  if (railPath) extensionArgs.push("--extension", railPath);
  const pi = spawn("pi", [
    "--mode", "rpc", "--no-session",
    "--no-extensions",            // no discovery: only the explicit loads
    ...extensionArgs,
    "--provider", "pecan-fixture",
    "--model", "fixture-v1",
  ], {
    stdio: ["pipe", "pipe", "pipe"],
    cwd: CWD,
    env: {
      ...process.env,
      PECAN_FIXTURE_PORT: String(port),
      PI_CODING_AGENT_DIR: AGENT,
      SUBAGENTS_FIXTURE_OUT: resultsPath,
    },
  });
  global.pi = pi;

  pi.stderr.on("data", (chunk) => { stderr += chunk.toString(); });

  const send = (obj) => pi.stdin.write(JSON.stringify(obj) + "\n");

  // The evidence, collected from the parent RPC stream.
  let spawnResult = null;      // tool_execution_end of subagent_spawn
  let waitStart = null;        // tool_execution_start of subagent_wait
  let waitResult = null;       // tool_execution_end of subagent_wait
  let settled = false;
  let observedRunning = false; // widget frame listed the child as running
  let terminalDone = null;     // widget terminal snapshot with status done
  let promptSent = false;
  let buf = "";

  function handle(event) {
    if (event.type === "agent_settled") { settled = true; return; }
    if (event.type === "extension_ui_request" && event.method === "setWidget" && event.widgetKey === WIDGET_KEY) {
      const line = Array.isArray(event.widgetLines) ? event.widgetLines[0] : undefined;
      if (typeof line !== "string") return;
      let snapshot;
      try { snapshot = JSON.parse(line); } catch { return; }
      const children = Array.isArray(snapshot.children) ? snapshot.children : [];
      for (const child of children) {
        if (child && child.status === "running" && typeof child.id === "string") {
          if (spawnResult && child.id === spawnResult.id && child.title === CHILD_TITLE) {
            observedRunning = true;
          }
        }
      }
      if (snapshot.terminal && snapshot.terminal.status === "done" && !terminalDone) {
        terminalDone = snapshot.terminal;
      }
      return;
    }
    if (event.type === "tool_execution_start" && event.toolName === "subagent_wait") {
      if (!waitStart) waitStart = event.args ?? null;
      return;
    }
    if (event.type === "tool_execution_end") {
      if (event.toolName === "subagent_spawn") {
        const details = event.result?.details ?? {};
        const text = Array.isArray(event.result?.content)
          ? event.result.content.map((b) => b.text ?? "").join("")
          : "";
        spawnResult = {
          id: details.id,
          title: details.title,
          harness: details.harness,
          text,
        };
        print(`   <- subagent_spawn -> ${details.id} [${details.harness}] ${details.title}`);
        print(`      ${text.split("\n")[0]}`);
        return;
      }
      if (event.toolName === "subagent_wait") {
        waitResult = {
          text: Array.isArray(event.result?.content)
            ? event.result.content.map((b) => b.text ?? "").join("")
            : "",
          details: event.result?.details ?? {},
        };
        print(`   <- subagent_wait returned (child settled)`);
        return;
      }
    }
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

  // Kick off the parent run once pi is up.
  setTimeout(() => {
    if (promptSent) return;
    promptSent = true;
    send({ type: "prompt", message: "Drive the subagent proof" });
    print("   -> sent prompt (parent: subagent_spawn -> subagent_wait -> settle)");
  }, 2000);

  function verify() {
    if (failed) return;
    if (!spawnResult) return fail("subagent_spawn result never appeared on the wire");
    if (!/^sa-\d+$/.test(spawnResult.id)) return fail(`unexpected subagent id: ${spawnResult.id}`);
    if (spawnResult.harness !== "pi") return fail(`child harness must be pi, got ${spawnResult.harness}`);
    if (!spawnResult.text.includes(`Spawned subagent ${spawnResult.id}`)) return fail("spawn result text is not the documented shape");

    print(`   verify  spawned:  ${spawnResult.id} ${spawnResult.title} (${spawnResult.harness})`);
    if (!observedRunning) {
      return fail(
        `never observed the child as RUNNING in a ${WIDGET_KEY} widget frame; ` +
        `expected a child frame with id ${spawnResult.id} + status "running"`,
      );
    }
    print(`   verify  running:  ${WIDGET_KEY} widget listed ${spawnResult.id} with status "running"`);

    if (!waitStart) return fail("subagent_wait never started (the parent must block on the child)");
    if (!waitResult) return fail("subagent_wait result never appeared");
    if (!terminalDone) return fail("the activity widget never published a done terminal snapshot");
    const expectedHeading = `## ${spawnResult.id} "${CHILD_TITLE}" finished`;
    if (!waitResult.text.includes(expectedHeading)) {
      return fail(`wait result must lead with ${expectedHeading}:\n${waitResult.text}`);
    }
    if (!waitResult.text.includes(CHILD_REPLY)) {
      return fail(`wait result must include the child output ${JSON.stringify(CHILD_REPLY)}:\n${waitResult.text}`);
    }
    if (terminalDone.id !== spawnResult.id || terminalDone.output !== CHILD_REPLY) {
      return fail(`terminal snapshot mismatch: ${JSON.stringify(terminalDone)}`);
    }
    print(`   verify  settled:  wait returned {status: done, output: ${JSON.stringify(CHILD_REPLY)}}`);

    // Settle ordering: agent_settled follows the wait result.
    if (!settled) return fail("parent never emitted agent_settled after the wait result");
    print("   verify  parent:   agent_settled after the one-child run");

    // Fixture report: the provider saw the same tool results the wire showed.
    const report = readFileSync(resultsPath, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
    const spawnLine = report.find((entry) => entry.kind === "parent-spawn-result");
    const waitLine = report.find((entry) => entry.kind === "parent-wait-result");
    if (!spawnLine || !spawnLine.text.includes(`Spawned subagent ${spawnResult.id}`)) {
      return fail(`fixture report must record the spawn result text with id ${spawnResult.id}`);
    }
    if (!waitLine || !waitLine.text.includes(CHILD_REPLY)) {
      return fail("fixture report must record the wait result text with the child output");
    }
    print(`   verify  report:   provider-observed spawn/wait results agree with the wire`);

    // The child really ran: its persisted session jsonl holds the prompt it
    // received and the reply it produced.
    const childSession = findChildSession(AGENT);
    if (!childSession) return fail("no child session jsonl under the staged agent dir");
    const transcript = readFileSync(childSession, "utf8");
    if (!transcript.includes(CHILD_PROMPT)) return fail("child transcript missing its prompt");
    if (!transcript.includes(CHILD_REPLY)) return fail("child transcript missing its final reply");
    if (!transcript.includes(`"name":"subagents: ${CHILD_TITLE}"`)) {
      return fail("child transcript missing the subagents session name");
    }
    print(`   verify  child:    session ${childSession.split("/").pop()} persisted prompt + ${JSON.stringify(CHILD_REPLY)}`);

    cleanup();
    print("   pi settled; one pi child spawned, observed running, and settled offline");
    process.exit(0);
  }

  const deadline = Date.now() + 60000;
  const poll = setInterval(() => {
    if (failed) return clearInterval(poll);
    if (settled && spawnResult && waitResult && terminalDone) return verify();
    if (Date.now() > deadline) {
      fail(
        "timed out waiting for the one-child lifecycle\n" +
        `  spawnResult=${JSON.stringify(spawnResult)}\n` +
        `  waitStart=${JSON.stringify(waitStart)}\n` +
        `  waitResult=${JSON.stringify(waitResult)}\n` +
        `  observedRunning=${observedRunning}\n` +
        `  terminalDone=${JSON.stringify(terminalDone)}\n` +
        `  settled=${settled}`,
      );
    }
  }, 100);

  pi.on("exit", (code) => {
    if (failed) return;
    if (code !== 0 && code !== null) fail(`pi exited with code ${code}`);
  });
}

/** Newest *.jsonl under the staged agent dir (the child's session file). */
function findChildSession(agentDir) {
  let newest = null;
  let newestMs = -1;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      const st = statSync(full);
      if (st.isDirectory()) walk(full);
      else if (name.endsWith(".jsonl") && st.mtimeMs > newestMs) {
        newest = full;
        newestMs = st.mtimeMs;
      }
    }
  };
  walk(agentDir);
  return newest;
}

function cleanup() {
  try { pi.kill(); } catch {}
  mock.close();
  const { rmSync } = require("node:fs");
  rmSync(CWD, { recursive: true, force: true });
  rmSync(AGENT, { recursive: true, force: true });
}

process.on("exit", (code) => {
  // Best-effort cleanup on every exit path (mock may still be listening).
  try { mock.close(); } catch {}
});
EOF