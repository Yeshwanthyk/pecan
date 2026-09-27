# Remote access over Tailscale

`pecan remote` makes a localhost-bound server reachable from a phone on the
tailnet. Tailscale handles HTTPS and transport, and pairing still gates every
request.

## Behaviors

- R1 read-only status: without `--enable`, it prints the tailnet URL and
  whether the port is `off`, `serving`, or in `conflict`. Tailscale config is
  not changed.
- R2 enable: `--enable` on an `off` port runs exactly
  `tailscale serve --bg --https=<port> http://127.0.0.1:<port>`. On a port
  already `serving`, it does nothing.
- R3 refusal: it exits nonzero, without calling serve, when the port proxies
  elsewhere, Tailscale is not `Running`, or the binary is missing.

## Drive

Never drive this against the real Tailscale without the user's go-ahead. Use a
fake binary through `PECAN_TAILSCALE=/path/to/script`, which must answer
`status --json` and `serve status --json` with canned JSON.

## Proof

- Scorecard row `remote-enable`: the fake records exactly one serve call.
- `cargo test -p pecan --test remote_fixture` covers R1–R3.

## Gotchas

- Behind `tailscale serve`, every request comes from 127.0.0.1, which is why
  loopback is never trusted.
- Use the same HTTPS port as the local port and never 443. Other services on
  this machine already hold many tailnet ports.
