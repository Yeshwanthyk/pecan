# Web push

A subscribed device hears about finished turns and waiting dialogs even with
Pecan closed. Pushes carry no content: the service worker fetches
`GET /api/push/pending` with the device cookie and shows those notices.

## Behaviors

- P1 subscribe: only a paired browser can `PUT /api/push/subscription`
  `{endpoint}`. The CLI gets 403. An endpoint outside the known push services
  gets 400.
- P2 trigger: `agent_settled` sends "Pi finished", and a blocking dialog sends
  "Pi needs your input". Each goes to every subscribed device with no open
  `/api/events` stream, as a POST with `Authorization: vapid t=<ES256 JWT>,
  k=<key>` and `Content-Length: 0`.
- P3 skip live: a device holding an open event stream is not pushed.
- P4 drain: `/api/push/pending` returns the queued notices once (at most 8
  per device, 1 hour TTL, collapsed by tag).
- P5 cleanup: a 404 or 410 from the push service deletes that subscription.
  Revoking a device deletes its subscription too.
- P6 test: `pecan push test` pushes every device (a browser's
  `POST /api/push/test` pushes only itself) and reports sent, removed and
  failed.

## Drive

Start `pecan serve` with `PECAN_PUSH_EXTRA_ORIGIN=http://127.0.0.1:<port>`
pointing at a local HTTP server that logs POSTs and answers 201 (or 410 for
P5). Pair a device with curl and a cookie jar, then PUT an endpoint under that
origin. Run a turn with `pecan session send`, then read the fake's log and
`/api/push/pending`.

## Proof

- Scorecard rows `push-turn` (P1, P2, P4), `push-skips-live` (P3) and
  `push-test` (P6).
- `cargo test -p pecan --test push_fixture` covers P1 and P4–P6, and verifies
  the VAPID signature against `/api/push/key`.
- Unit tests in `server/push.rs` cover the JWT, the endpoint allowlist, the
  queue limits and the live-stream counting.

## Gotchas

- The browser pane cannot grant notification permission, so the real
  `pushManager.subscribe` needs a phone or desktop browser. On iPhone that
  means the Home Screen app over HTTPS.
- A real subscription's endpoint is on a public push service, so a real
  `pecan push test` reaches Google, Apple or Mozilla.
