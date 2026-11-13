# NodeWatch

A lightweight, self-hosted, **read-only** system monitoring dashboard for Linux.
One static binary (frontend embedded), no database, no JavaScript framework.

- **CPU:** overall and per-core utilisation
- **Memory:** total / used / available RAM and swap
- **Storage:** mount points with total / used / available space
- **System:** hostname, OS, kernel, architecture, uptime
- Dark, responsive dashboard that refreshes every 2.5 s, with loading, stale and offline states
- Token authentication is **always required**, even on loopback

NodeWatch currently monitors the host it runs on. It reads `/proc`, `/sys` and
filesystem statistics only; it never executes commands or writes anywhere.

> **Security in one paragraph.** NodeWatch speaks plain HTTP. On a LAN, do not
> bind it to a network address directly: keep the default loopback bind and put a
> TLS reverse proxy in front of it, or use an SSH tunnel. See
> [Accessing it from another device](#accessing-it-from-another-lan-device).

## Requirements

- Linux (uses `/proc`, `/sys` and `/dev/urandom`), x86_64 or any architecture Rust supports
- Rust **1.95 or newer** to build (a requirement of the `sysinfo` crate)

## Quick start

```sh
cargo build --release

# Generate an access token (at least 32 characters is enforced)
export NODEWATCH_TOKEN="$(openssl rand -hex 32)"
echo "$NODEWATCH_TOKEN"            # you will paste this into the login page

./target/release/nodewatch
# INFO nodewatch: listening on http://127.0.0.1:8080
```

Open <http://127.0.0.1:8080>, paste the token, and the dashboard appears.
Without a token the process refuses to start and says why.

## Configuration

Everything is configured through environment variables. Invalid values stop the
process at startup with a message naming the variable.

| Variable | Default | Description |
|---|---|---|
| `NODEWATCH_TOKEN` | – | Access token, at least 32 characters. Prefer `NODEWATCH_TOKEN_FILE`: environment variables are visible to root and in `/proc/<pid>/environ`. |
| `NODEWATCH_TOKEN_FILE` | – | Path to a file containing the token (surrounding whitespace is trimmed). Setting both variables is an error. |
| `NODEWATCH_BIND` | `127.0.0.1:8080` | Socket address to listen on, e.g. `127.0.0.1:8080` or `192.168.1.10:8080`. IPv6 is written `[::1]:8080`. |
| `NODEWATCH_COOKIE_SECURE` | `false` | Adds the `Secure` flag to the session cookie. Set to `true` when browsers reach NodeWatch over HTTPS (a TLS proxy). With `true`, login over plain HTTP will not work. |
| `NODEWATCH_SESSION_TTL_SECS` | `43200` (12 h) | Session lifetime. Clamped to 60 s … 30 days. |
| `NODEWATCH_REFRESH_SECS` | `2` | How often metrics are sampled. Clamped to 1 … 60 s. |
| `RUST_LOG` | `info` | Log filter ([`tracing` EnvFilter syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)). |

There is deliberately no "disable authentication" option.

## API

All responses are JSON. Every `/api/*` response carries `Cache-Control: no-store`.

| Method & path | Auth | Description |
|---|---|---|
| `GET /api/health` | none | Liveness probe. Returns nothing about the host. |
| `GET /api/stats` | session cookie or Bearer token | Latest metrics snapshot. |
| `POST /api/login` | none | Body `{"token": "..."}` (must be `application/json`). Sets the session cookie. |
| `POST /api/logout` | none | Body `{}` (must be `application/json`). Ends the session and clears the cookie. |
| `GET /`, `GET /app.js` | session cookie | The dashboard. Redirects to `/login` without a session. |
| `GET /login`, `/login.js`, `/style.css` | none | Login page and shared stylesheet. |

Status codes: `200` success · `401` missing/invalid credentials · `404` unknown path ·
`413` body over 4 KiB · `415` wrong content type · `503` on `/api/stats` before the first
sample is ready (a fraction of a second after startup).

### Examples

```sh
T=…your token…

# Health (no auth)
curl -s http://127.0.0.1:8080/api/health
# {"status":"ok"}

# Stats with a Bearer token (good for scripts)
curl -s -H "Authorization: Bearer $T" http://127.0.0.1:8080/api/stats | jq

# Stats with a session, the way the browser does it
curl -s -c jar -X POST -H 'content-type: application/json' \
     -d "{\"token\":\"$T\"}" http://127.0.0.1:8080/api/login
# {"status":"ok"}
curl -s -b jar http://127.0.0.1:8080/api/stats | jq
```

Without credentials:

```
HTTP/1.1 401 Unauthorized
www-authenticate: Bearer

{"error":"authentication required"}
```

A `GET /api/stats` response (host values are illustrative; only the first two
CPU cores and disks are shown):

```json
{
  "collected_at_ms": 1794559065698,
  "interval_secs": 2,
  "system": {
    "hostname": "node01",
    "os": "Gentoo 2.18",
    "kernel": "7.3.3-gentoo-dist",
    "uptime_secs": 302233,
    "cpu_arch": "x86_64"
  },
  "cpu": {
    "usage_percent": 8.5,
    "core_count": 28,
    "brand": "Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz",
    "cores": [
      { "name": "cpu0", "usage_percent": 13.0, "frequency_mhz": 1210 },
      { "name": "cpu1", "usage_percent": 4.5, "frequency_mhz": 1200 }
    ]
  },
  "memory": {
    "total_bytes": 67230658560,
    "used_bytes": 3859390464,
    "available_bytes": 63371268096,
    "usage_percent": 5.7,
    "swap_total_bytes": 8589930496,
    "swap_used_bytes": 0
  },
  "disks": [
    {
      "mount_point": "/",
      "name": "/dev/nvme0n1p3",
      "file_system": "ext4",
      "total_bytes": 493409042432,
      "used_bytes": 206282145792,
      "available_bytes": 287126896640,
      "usage_percent": 41.8,
      "read_only": false,
      "removable": false
    }
  ]
}
```

Notes on the data:

- Sizes are raw **bytes**; percentages are 0–100 with one decimal.
- `collected_at_ms` is the Unix time (ms) the sample finished. The dashboard
  compares it across polls to detect a stalled sampler.
- A metric the OS cannot provide is `null` instead of failing the request (for
  example `system.hostname`).
- A disk's `used_bytes` is `total − available`, so it includes blocks reserved
  for root, matching `df`.
- Pseudo filesystems (tmpfs, overlay, squashfs, loop devices, …) are hidden, and
  each mount point is listed once.

## Accessing it from another LAN device

The access token and session cookie travel in **cleartext over plain HTTP**:
anyone on the same network segment can read them. Pick one of these instead of
exposing the HTTP port to the LAN.

### Option A: SSH tunnel (simplest, nothing to install)

NodeWatch stays on `127.0.0.1`. From the other device:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@server
```

Then browse to <http://127.0.0.1:8080> on that device. Traffic is encrypted by SSH.
Keep `NODEWATCH_COOKIE_SECURE=false` here.

### Option B: TLS reverse proxy

NodeWatch stays on `127.0.0.1:8080`; the proxy listens on the LAN with HTTPS.

1. Set `NODEWATCH_COOKIE_SECURE=true`.
2. Use [`deploy/Caddyfile.example`](deploy/Caddyfile.example) (Caddy) or
   [`deploy/nginx.conf.example`](deploy/nginx.conf.example) (nginx).
3. Make the certificate trusted by your devices. With Caddy's `tls internal`,
   install Caddy's local root CA on each client (`caddy trust` does this on the proxy
   host itself; for other devices copy the `root.crt` Caddy generated under its data
   directory, see the Caddy docs for the exact path on your install). Alternatively use a certificate from
   your own LAN CA, or a real domain with an ACME DNS challenge.
4. Allow the proxy port through the firewall for the LAN only (below).

The nginx example was tested here end to end (login, `Secure` cookie, dashboard
and redirects through the proxy). The Caddy example was not run (Caddy is not
installed on the development host).

Do not forward port 8080 or 443 from your router to the internet. NodeWatch is
designed for a trusted LAN or VPN, not public exposure.

### Binding directly to a LAN address (not recommended)

`NODEWATCH_BIND=192.168.1.10:8080` works, but exposes the token in cleartext.
The server logs a warning at startup for any non-loopback bind. Use a specific
interface address rather than `0.0.0.0`, and apply the firewall rules below.

## Firewall

Allow only your LAN subnet to reach the port that is exposed (the proxy's 443, or
8080 for a direct bind), and nothing else.

**nftables**: see [`deploy/nftables.example`](deploy/nftables.example) (syntax-checked).
Load it with `nft -f deploy/nftables.example`. It creates its own table that runs
just before your normal chains. An `accept` in one table does not override a
`drop` in another, so if your host already uses a default-drop firewall, add the
same allow rule there.

**ufw**:

```sh
sudo ufw allow from 192.168.1.0/24 to any port 443 proto tcp
```

When using a proxy, do **not** open port 8080 at all; it listens on loopback only.

## Running as a service

[`deploy/nodewatch.service`](deploy/nodewatch.service) is a hardened systemd unit:

```sh
sudo install -m 0755 target/release/nodewatch /usr/local/bin/nodewatch
sudo install -d -m 0700 /etc/nodewatch
openssl rand -hex 32 | sudo install -m 0600 /dev/stdin /etc/nodewatch/token
sudo cp deploy/nodewatch.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now nodewatch
journalctl -u nodewatch -f
```

It runs as a transient unprivileged user (`DynamicUser`), receives the token as a
systemd credential (never in the environment), drops all capabilities, makes the
filesystem read-only, and limits system calls and address families.
`systemd-analyze security --offline` rates it **1.5 OK**.
The unit file passes `systemd-analyze verify`, but it has not been started under
systemd on the development host (that needs root); check `systemctl status` and the
dashboard's disk list after installing.

**Changing the token:** write the new token to `/etc/nodewatch/token` and run
`systemctl restart nodewatch`. Sessions live in memory, so every browser is signed out.

## Security model

- **Authentication is mandatory.** The process will not start without a token of at
  least 32 characters; there is no bypass, even on loopback (a loopback bind is
  reachable from the LAN through a reverse proxy).
- **Browser sessions:** logging in exchanges the token for a random 256-bit session id
  in an `HttpOnly`, `SameSite=Strict` cookie (`Secure` when configured). The token is
  never stored in a cookie, in the page, or in browser storage, and is never echoed
  by the API. Sessions expire (default 12 h), are limited to 1024, and are cleared by
  logout or restart.
- **Scripts** may send `Authorization: Bearer <token>` instead.
- **Comparison and throttling:** tokens are compared in constant time; a failed login
  waits 0.5 s before responding and is logged with the peer address (never the token).
- **CSRF:** the cookie is `SameSite=Strict`, and login/logout accept only
  `application/json`, which cross-site forms cannot send.
- **CORS:** no CORS headers are emitted. The dashboard is same-origin, so browsers
  block cross-origin reads of the API.
- **Headers:** `Content-Security-Policy: default-src 'self'; frame-ancestors 'none'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`. The frontend uses no
  inline scripts or styles, and writes server data with `textContent` only.
- **Secrets are never logged.** The token type redacts itself in `Debug` output, and
  request headers and bodies are not logged.
- **Read-only:** no shell, command execution or file writes. The only paths read are
  `/proc`, `/sys`, mount statistics, `/dev/urandom` and the optional token file.
- **Static assets are compiled into the binary:** there is no file serving, so no
  path traversal.
- Request bodies are capped at 4 KiB.

### Known limitations

- **No built-in TLS.** Use a proxy or SSH tunnel (above).
- **Failed-login log lines show the proxy's address** (`127.0.0.1`) when running behind a
  reverse proxy, because NodeWatch logs the TCP peer and does not trust
  `X-Forwarded-For`.
- **No account lockout.** The 32-character minimum token and the response delay make
  guessing impractical, but a determined LAN attacker is not locked out.
- **Sessions are in memory:** a restart signs everyone out; multiple instances do not
  share sessions.
- A filesystem that hangs (for example a dead network mount) can stall the sampler.
  The server stays responsive and the dashboard shows a **Stale data** warning.

## Development

```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

```
src/
  main.rs            startup, tracing, graceful shutdown
  config.rs          environment configuration (injectable for tests), token secrecy
  auth.rs            login/logout, sessions, auth middleware
  collector/         sysinfo -> serializable snapshot (cpu, memory, disks, system)
                     and the background sampler
  api/               router, security headers, handlers
static/              index.html, app.js, login.html, login.js, style.css (embedded at build time)
deploy/              systemd unit, nftables, Caddy and nginx examples
```

Design notes:

- **A background sampler owns all OS access.** CPU usage needs two readings spaced
  apart, so one task refreshes `sysinfo` on a timer (inside `spawn_blocking`) and
  publishes an immutable `Snapshot`. Request handlers only clone an `Arc`, so client
  load cannot increase collection work and the Tokio runtime is never blocked.
- Frontend edits require a rebuild, because the assets are embedded.

### Where future features fit

| Roadmap item | Where it plugs in |
|---|---|
| Historical metrics, graphs, persistent storage | Keep a ring buffer or store beside the snapshot in the sampler; add a `/api/history` route. |
| Network, process, service, Docker monitoring | A new module in `collector/` that fills a new field on `Snapshot`; enable the matching `sysinfo` feature. |
| Alert thresholds | Evaluate the new snapshot in the sampler task; handlers stay unchanged. |
| Multi-host | An agent exposing `/api/stats` per host, and a collector that polls them with Bearer tokens. |
