# Pairing

A phone or browser must be paired before it can see or drive anything. The CLI
authenticates with the token that `pecan serve` wrote.

## Behaviors

- P1 unpaired: every `/api/*` route, including `/api/events`, returns
  401 with `{"code":"unpaired"}`, and the web app shows the pair screen.
- P2 bad code: `POST /api/pair` with a wrong code returns 401 and sets no cookie.
- P3 pair: a code from `pecan pair` sets the device cookie, after which
  bootstrap returns 200. Codes are single use; case and dashes are ignored.
- P4 CLI-only: a device cookie gets 403 on `/api/devices` and `/api/pair/code`.
- P5 revoke: `pecan devices revoke <id>` makes that cookie get 401 and ends
  the device's open event stream.

## User entry points

- CLI: `pecan pair [--url <base>] [--json]`, `pecan devices [--json]`,
  `pecan devices revoke <id>`.
- Phone UI: the `pair-screen` code field, or a `…/?pair=CODE` link, which is
  removed from the URL once used.

## Drive

1. `curl -s -o /dev/null -w '%{http_code}' $PECAN_SERVER_URL/api/bootstrap` → `401`.
2. `CODE=$(target/debug/pecan pair --json | jq -r .code)`.
3. `curl -c jar -H 'content-type: application/json' -d "{\"code\":\"$CODE\"}" $PECAN_SERVER_URL/api/pair`
   → `.device.id`.
4. `curl -b jar $PECAN_SERVER_URL/api/bootstrap` → 200; `curl -b jar …/api/devices` → 403.
5. `target/debug/pecan devices revoke <id>`, then `curl -b jar …/api/bootstrap` → 401.
6. UI: smoke step `pair` checks that a wrong code shows the alert and that
   the right code reaches `empty-state`.

## Proof

- Scorecard rows: `unpaired-401`, `sse-unpaired-401`, `pair-bad-code`,
  `pair-ok`, `devices-list`, `device-cli-only-403`, `revoke-401`.
- UI row `pair`, plus `ui/0-pair-rejected.png`.
- `cargo test -p pecan --test pairing_fixture` covers the stream closing on
  revoke.

## Gotchas

- Codes live only in memory, so restarting the server invalidates unused ones.
  Paired devices persist in `pecan.db`.
- Streams stay open over keep-alive; to see one end, send `Connection: close`.
