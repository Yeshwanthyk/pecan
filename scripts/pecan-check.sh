#!/usr/bin/env bash
#
# End-to-end Pecan scorecard, driven entirely through the `pecan` CLI.
#
# Starts an isolated `pecan serve` (temp agent dir, free port) whose Pi
# workers load the deterministic fixture model (scripts/fixture-model.ts),
# then drives every session flow and prints a JSON scorecard with pass/fail
# and client-measured timings. Nothing touches ~/.pi.
#
# Usage:
#   scripts/pecan-check.sh [--out <scorecard.json>] [--keep] [--ui]
#
#   --out   also write the scorecard to this path
#   --keep  leave the scratch dir (server log, per-step JSON) for inspection
#   --ui    also run the phone-viewport browser smoke (web/scripts/ui-smoke.mjs,
#           needs Google Chrome); screenshots land in <scratch>/ui
#
# Exit status: 0 when every scenario passes, 1 otherwise.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT=""
KEEP=0
UI=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --ui) UI=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

for tool in pi jq cargo; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 2; }
done

cargo build --quiet --manifest-path "$REPO_ROOT/Cargo.toml" -p pecan
PECAN="$REPO_ROOT/target/debug/pecan"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/pecan-check.XXXXXX")"
AGENT_DIR="$SCRATCH/agent"
WORKDIR="$SCRATCH/project"
mkdir -p "$AGENT_DIR/sessions" "$WORKDIR"
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
export PECAN_SERVER_URL="http://127.0.0.1:$PORT"
export PECAN_AGENT_DIR="$AGENT_DIR"
export PECAN_PI_ARGS="--no-extensions --extension $REPO_ROOT/scripts/fixture-model.ts --provider pecan-fixture --model fixture-v1"

# Fake web push service (admitted via PECAN_PUSH_EXTRA_ORIGIN): logs each
# request line and its Authorization header, answers 201.
PUSH_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
export PECAN_PUSH_EXTRA_ORIGIN="http://127.0.0.1:$PUSH_PORT"
python3 - "$PUSH_PORT" "$SCRATCH/push-log" <<'PY' &
import http.server, sys
log = sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        with open(log, "a") as f:
            f.write(f"{self.path}\t{self.headers.get('Authorization', '')}\n")
        self.send_response(201); self.send_header("Content-Length", "0"); self.end_headers()
    def log_message(self, *args): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
PUSH_PID=$!

SERVER_PID=""
cleanup() {
  kill "$PUSH_PID" 2>/dev/null || true
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [[ "$KEEP" == 0 ]]; then rm -rf "$SCRATCH"; else echo "scratch kept: $SCRATCH" >&2; fi
}
trap cleanup EXIT

"$PECAN" add "$WORKDIR" >/dev/null
"$PECAN" serve --port "$PORT" --no-open >"$SCRATCH/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 100); do
  [[ -s "$AGENT_DIR/pecan/cli-token" ]] && curl -fsS -o /dev/null "$PECAN_SERVER_URL/" 2>/dev/null && break
  sleep 0.1
done
# Every API call needs a paired device or the CLI token `serve` just wrote.
AUTH=(-H "Authorization: Bearer $(cat "$AGENT_DIR/pecan/cli-token" 2>/dev/null)")
curl -fsS "${AUTH[@]}" "$PECAN_SERVER_URL/api/bootstrap" >/dev/null || { cat "$SCRATCH/server.log" >&2; exit 1; }

RESULTS="$SCRATCH/results.jsonl"
: >"$RESULTS"

# record <name> <pass:true|false> <detail-json>
record() {
  jq -cn --arg name "$1" --argjson pass "$2" --argjson detail "$3" \
    '{name: $name, pass: $pass} + $detail' >>"$RESULTS"
}

# check <name> <jq-predicate> <cli args...>: run the CLI with --json, save
# its output, and pass when the predicate holds.
check() {
  local name="$1" predicate="$2"; shift 2
  local file="$SCRATCH/$name.json"
  if "$PECAN" "$@" --json --timeout 60 >"$file" 2>"$SCRATCH/$name.err"; then
    local pass; pass="$(jq "$predicate" "$file")"
    record "$name" "$pass" "$(jq -c '{turn: (.turn // null | if . then {outcome, firstTokenMs, totalMs, tools, error} else null end), createMs: (.createMs // null)}' "$file")"
  else
    record "$name" false "$(jq -cn --arg err "$(cat "$SCRATCH/$name.err")" '{error: $err}')"
  fi
}

check create '.id | length > 0' session new --cwd "$WORKDIR"
ID="$(jq -r '.id // empty' "$SCRATCH/create.json")"
if [[ -z "$ID" ]]; then
  jq -s '{pass: false, scenarios: .}' "$RESULTS"
  exit 1
fi

check echo '.turn.outcome == "settled" and .turn.text == "Echo: hello pecan"' \
  session send "$ID" hello pecan
check tool '.turn.tools == ["bash"] and (.turn.text | contains("fixture-tool-ok"))' \
  session send "$ID" /tool
check think '.turn.text == "Thought about: deeply"' session send "$ID" /think deeply
check rich '.turn.text | contains("```mermaid")' session send "$ID" /rich
check error '.turn.error == "fixture provider error"' session send "$ID" /error
"$PECAN" session send "$ID" /slow --no-wait --json >/dev/null
# A plain send while streaming must be refused (409).
if "$PECAN" session send "$ID" too eager --no-wait --json >/dev/null 2>&1; then
  record busy-send-rejected false '{}'
else
  record busy-send-rejected true '{}'
fi
check steer '.turn.outcome == "settled" and .turn.text == "Echo: change course"' \
  session send "$ID" change course --mode steer
"$PECAN" session send "$ID" /slow --no-wait --json >/dev/null
check queue '.turn.outcome == "settled" and .turn.text == "Echo: after that"' \
  session send "$ID" after that --mode queue
"$PECAN" session send "$ID" /slow --no-wait --json >/dev/null
sleep 0.5
check abort '.aborted == true' session abort "$ID"
check confirm-opens '.turn.outcome == "waiting-for-user" and .turn.dialog.method == "confirm"' \
  session send "$ID" /confirm ship it
REQUEST="$(jq -r '.turn.dialog.id // empty' "$SCRATCH/confirm-opens.json")"
check confirm-answered '.turn.outcome == "settled" and (.turn.text | contains("confirmed"))' \
  session respond "$ID" "${REQUEST:-missing}" --confirm
check state '.state.isStreaming == false' session state "$ID"

check health '.status == "ok" and (.version | length > 0)' health

# Pairing: nothing reaches the API without the CLI token or a device cookie.
# status_of <curl args...>: HTTP status, body saved to $SCRATCH/last.json.
status_of() { curl -sS -o "$SCRATCH/last.json" -w '%{http_code}' "$@" || true; }
JAR="$SCRATCH/cookies.txt"
STATUS="$(status_of "$PECAN_SERVER_URL/api/bootstrap")"
record unpaired-401 "$([[ "$STATUS" == 401 ]] && jq '.code == "unpaired"' "$SCRATCH/last.json" || echo false)" "{\"status\": $STATUS}"
STATUS="$(status_of --max-time 2 "$PECAN_SERVER_URL/api/events")"
record sse-unpaired-401 "$([[ "$STATUS" == 401 ]] && echo true || echo false)" "{\"status\": $STATUS}"
STATUS="$(status_of -c "$JAR" -H 'content-type: application/json' -d '{"code":"ZZZZ-ZZZZ"}' "$PECAN_SERVER_URL/api/pair")"
record pair-bad-code "$([[ "$STATUS" == 401 ]] && ! grep -q pecan_ "$JAR" 2>/dev/null && echo true || echo false)" "{\"status\": $STATUS}"
CODE="$("$PECAN" pair --json | jq -r '.code')"
PAIR_STATUS="$(status_of -c "$JAR" -H 'content-type: application/json' -d "{\"code\":\"$CODE\"}" "$PECAN_SERVER_URL/api/pair")"
DEVICE_ID="$(jq -r '.device.id // empty' "$SCRATCH/last.json")"
STATUS="$(status_of -b "$JAR" "$PECAN_SERVER_URL/api/bootstrap")"
record pair-ok "$([[ "$PAIR_STATUS" == 200 && "$STATUS" == 200 && -n "$DEVICE_ID" ]] && echo true || echo false)" \
  "{\"pairStatus\": $PAIR_STATUS, \"bootstrapStatus\": $STATUS}"
check devices-list "any(.devices[]; .id == \"$DEVICE_ID\")" devices
STATUS="$(status_of -b "$JAR" "$PECAN_SERVER_URL/api/devices")"
record device-cli-only-403 "$([[ "$STATUS" == 403 ]] && echo true || echo false)" "{\"status\": $STATUS}"
"$PECAN" devices revoke "$DEVICE_ID" --json >/dev/null 2>&1 || true
STATUS="$(status_of -b "$JAR" "$PECAN_SERVER_URL/api/bootstrap")"
record revoke-401 "$([[ "$STATUS" == 401 ]] && echo true || echo false)" "{\"status\": $STATUS}"

# `pecan remote --enable` against a fake tailscale: one serve call, for our port.
FAKE_TS="$SCRATCH/fake-tailscale"
cat >"$FAKE_TS" <<'SH'
#!/bin/sh
echo "$*" >> "$(dirname "$0")/tailscale-calls"
case "$1 $2" in
  "status --json") printf '{"BackendState":"Running","Self":{"DNSName":"box.tail0.ts.net."}}' ;;
  "serve status") printf '{}' ;;
esac
SH
chmod +x "$FAKE_TS"
if PECAN_TAILSCALE="$FAKE_TS" "$PECAN" remote --port 7700 --enable --json >"$SCRATCH/remote.json" 2>"$SCRATCH/remote.err"; then
  SERVES="$(grep -c '^serve --bg --https=7700 http://127.0.0.1:7700$' "$SCRATCH/tailscale-calls" || true)"
  record remote-enable "$(jq --argjson serves "$SERVES" '.state == "serving" and .url == "https://box.tail0.ts.net:7700" and $serves == 1' "$SCRATCH/remote.json")" \
    "$(jq -c --argjson serves "$SERVES" '{url, state, serveCalls: $serves}' "$SCRATCH/remote.json")"
else
  record remote-enable false "$(jq -cn --arg err "$(cat "$SCRATCH/remote.err")" '{error: $err}')"
fi

# Web push: a finished turn pushes (VAPID-signed) to a subscribed device with
# no open stream, and its service worker drains "Pi finished" exactly once.
PUSH_JAR="$SCRATCH/push-cookies.txt"
CODE="$("$PECAN" pair --json | jq -r '.code')"
status_of -c "$PUSH_JAR" -H 'content-type: application/json' -d "{\"code\":\"$CODE\"}" "$PECAN_SERVER_URL/api/pair" >/dev/null
SUB_STATUS="$(status_of -b "$PUSH_JAR" -X PUT -H 'content-type: application/json' \
  -d "{\"endpoint\":\"$PECAN_PUSH_EXTRA_ORIGIN/ok/check\"}" "$PECAN_SERVER_URL/api/push/subscription")"
BAD_STATUS="$(status_of -b "$PUSH_JAR" -X PUT -H 'content-type: application/json' \
  -d '{"endpoint":"https://evil.example/push"}' "$PECAN_SERVER_URL/api/push/subscription")"
"$PECAN" session send "$ID" push me --json --timeout 60 >/dev/null 2>&1 || true
for _ in $(seq 1 50); do [[ -s "$SCRATCH/push-log" ]] && break; sleep 0.1; done
PUSHES="$(grep -c $'^/ok/check\tvapid t=' "$SCRATCH/push-log" 2>/dev/null || true)"
curl -sS -b "$PUSH_JAR" "$PECAN_SERVER_URL/api/push/pending" >"$SCRATCH/push-pending.json"
AGAIN="$(curl -sS -b "$PUSH_JAR" "$PECAN_SERVER_URL/api/push/pending" | jq '.notifications | length')"
record push-turn "$(jq --arg id "$ID" --argjson pushes "${PUSHES:-0}" --argjson again "$AGAIN" \
  --argjson sub "$SUB_STATUS" --argjson bad "$BAD_STATUS" \
  '$sub == 200 and $bad == 400 and $pushes >= 1 and $again == 0
   and any(.notifications[]; .title == "Pi finished" and .tag == ("pecan:" + $id))' "$SCRATCH/push-pending.json")" \
  "$(jq -c --argjson pushes "${PUSHES:-0}" --argjson sub "$SUB_STATUS" --argjson bad "$BAD_STATUS" \
    '{subscribe: $sub, badEndpoint: $bad, pushes: $pushes, pending: [.notifications[].title]}' "$SCRATCH/push-pending.json")"
# While that device holds a live stream it hears events in-page: no push.
curl -sN -b "$PUSH_JAR" --max-time 30 "$PECAN_SERVER_URL/api/events" >/dev/null 2>&1 &
STREAM_PID=$!
sleep 0.5
"$PECAN" session send "$ID" watching live --json --timeout 60 >/dev/null 2>&1 || true
sleep 0.5
LIVE_PUSHES="$(grep -c '^/ok/check' "$SCRATCH/push-log" 2>/dev/null || true)"
kill "$STREAM_PID" 2>/dev/null || true
wait "$STREAM_PID" 2>/dev/null || true
record push-skips-live "$([[ "$LIVE_PUSHES" == "${PUSHES:-0}" ]] && echo true || echo false)" \
  "{\"pushesBefore\": ${PUSHES:-0}, \"pushesAfter\": ${LIVE_PUSHES:-0}}"
check push-test '.sent >= 1 and .failed == 0' push test

# A retried prompt with the same Idempotency-Key must not run twice.
check idem-first '.turn.outcome == "settled" and .turn.text == "Echo: idem once"' \
  session send "$ID" idem once --idempotency-key pecan-check-idem-1
if "$PECAN" session send "$ID" idem once --idempotency-key pecan-check-idem-1 --no-wait --json \
  >"$SCRATCH/idem-retry.json" 2>"$SCRATCH/idem-retry.err"; then
  COUNT="$(curl -fsS "${AUTH[@]}" "$PECAN_SERVER_URL/api/session/$ID" | jq '[.. | strings | select(. == "idem once")] | length')"
  record idem-retry "$([[ "$COUNT" == 1 ]] && echo true || echo false)" "{\"userTurns\": $COUNT}"
else
  record idem-retry false "$(jq -cn --arg err "$(cat "$SCRATCH/idem-retry.err")" '{error: $err}')"
fi

# sse_frames <query>: the first second of the event stream as JSON frames.
sse_frames() {
  # curl exits 28 when --max-time cuts the endless stream; that is expected.
  { curl -sN --max-time 1 "${AUTH[@]}" "$PECAN_SERVER_URL/api/events$1" 2>/dev/null || true; } |
    awk '/^event:/{e=$2} /^id:/{i=$2} /^data:/{sub(/^data: ?/,""); printf "{\"event\":\"%s\",\"id\":\"%s\",\"data\":%s}\n", e, i, $0; e="message"; i=""}' |
    jq -cs '.'
}
REPLAY="$(sse_frames '?after=0')"
record sse-replay "$(jq '.[0].event == "ready" and .[0].data.reset == false and .[0].data.replayed > 0 and (.[1].id | length > 0)' <<<"$REPLAY")" \
  "$(jq -c '{frames: length, ready: .[0].data}' <<<"$REPLAY")"
FUTURE="$(sse_frames '?after=999999999')"
record sse-reset "$(jq '.[0].event == "ready" and .[0].data.reset == true' <<<"$FUTURE")" \
  "$(jq -c '{ready: .[0].data}' <<<"$FUTURE")"

# A Pi worker that dies must clear the session's pending dialog and publish
# worker_exit instead of leaving the UI "running" forever.
"$PECAN" session send "$ID" /confirm dies --no-wait --json >/dev/null
for _ in $(seq 1 50); do
  [[ "$(curl -fsS "${AUTH[@]}" "$PECAN_SERVER_URL/api/session/$ID/asks" | jq '.asks | length')" -gt 0 ]] && break
  sleep 0.1
done
BEFORE="$(sse_frames '' | jq '.[0].data.head')"
# Pi renames its process title, so match workers as the server's children.
WORKER_PIDS="$(pgrep -P "$SERVER_PID" | tr '\n' ' ' || true)"
[[ -n "$WORKER_PIDS" ]] && kill $WORKER_PIDS 2>/dev/null
for _ in $(seq 1 50); do
  [[ "$(curl -fsS "${AUTH[@]}" "$PECAN_SERVER_URL/api/session/$ID/asks" | jq '.asks | length')" == 0 ]] && break
  sleep 0.1
done
EXIT_FRAMES="$(sse_frames "?after=$BEFORE")"
ASKS="$(curl -fsS "${AUTH[@]}" "$PECAN_SERVER_URL/api/session/$ID/asks" | jq '.asks | length')"
record worker-exit "$(jq --argjson asks "$ASKS" 'any(.[]; .data.event.type? == "worker_exit") and $asks == 0' <<<"$EXIT_FRAMES")" \
  "$(jq -cn --argjson asks "$ASKS" --arg killed "$WORKER_PIDS" '{pendingAsks: $asks, killed: $killed}')"

if [[ "$UI" == 1 ]]; then
  if UI_JSON="$(node "$REPO_ROOT/web/scripts/ui-smoke.mjs" --url "$PECAN_SERVER_URL" \
    --pair "$("$PECAN" pair --json | jq -r '.code')" --out "$SCRATCH/ui" 2>"$SCRATCH/ui.err")"; then
    record ui-phone true "$(jq -c '{steps}' <<<"$UI_JSON")"
  else
    record ui-phone false "$(jq -c '{steps}' <<<"${UI_JSON:-{\"steps\":[]\}}" 2>/dev/null || echo '{}')"
  fi
fi

SCORECARD="$(jq -s '{pass: all(.[]; .pass), passed: map(select(.pass)) | length, total: length, scenarios: .}' "$RESULTS")"
echo "$SCORECARD"
[[ -n "$OUT" ]] && echo "$SCORECARD" >"$OUT"
[[ "$(jq '.pass' <<<"$SCORECARD")" == true ]]
