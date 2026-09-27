# Resilience: flaky phone links

Proves a phone that drops its connection mid-turn neither loses events nor
double-sends. All rows run in `scripts/pecan-check.sh`; the commands below
reproduce them by hand against the Launch server.

## Health

```bash
target/debug/pecan health --json            # .status == "ok", .version set
target/debug/pecan health --refresh --json   # re-probes `pi` instead of the cache
```

Failure path: launch with `PATH` lacking `pi` and the status is not `ok`; the
UI shows the health banner under the header.

## Retry-safe send

```bash
pecan session send "$ID" idem once --idempotency-key k-12345678 --json
pecan session send "$ID" idem once --idempotency-key k-12345678 --no-wait --json
curl -fsS "$PECAN_SERVER_URL/api/session/$ID" | jq '[.. | strings | select(. == "idem once")] | length'
```

Evidence: the count is `1`. Keys are 8–128 chars of `[A-Za-z0-9_-]`; anything
else is a 400, and a duplicate while the first is still running is a 409. A failed first attempt is not remembered, so its retry runs.

## SSE replay

```bash
curl -sN --max-time 1 "$PECAN_SERVER_URL/api/events?after=0"          # ready.reset == false, replayed > 0, frames carry id:
curl -sN --max-time 1 "$PECAN_SERVER_URL/api/events?after=999999999"  # ready.reset == true
```

`Last-Event-ID` works the same as `after`. On `reset` the web client refetches
the index and open thread instead of trusting deltas.

## Worker death

Kill the Pi worker while a `/confirm` is pending: `worker_exit` is published and
`session asks` returns an empty list (row `worker-exit`).

## Attention (browser only)

With the tab hidden, a finished turn or a new dialog prefixes the title with
`(N)`, plays a chime (Settings → sound), and shows a system notification when
enabled in Settings and permitted. Visible tabs get none of these.
