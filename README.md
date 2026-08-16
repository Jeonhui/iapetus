# Iapetus

A persistent virtual desktop for AI agents — one a human and an agent **co-own**
and drive through the same screen.

An agent controls a real Linux desktop through an OS-agnostic Computer API
(click, type, screenshot, launch apps, run shell commands). A person can open
that same desktop in a browser at any time, watch what the agent is doing, and
take over with one click. Both hold root-equivalent authority; what is
arbitrated is not authority but who has the keyboard right now.

> Status: the guest daemon, the stream gateway, the control-lease arbitration,
> and token issuance/verification are implemented and tested end to end. Desktop
> provisioning (Firecracker), the WebRTC media path, and the rest of the REST
> API are not yet built — see [Status](#status). The full design is in
> [`docs/PRD.md`](docs/PRD.md).

## Quickstart

Requires Docker with Compose.

```bash
docker compose up --build
```

Then open **<http://localhost:8080/?token=dev-write>** — you are looking at a
live virtual desktop with a browser on it. Click into the page, type, scroll:
your input is converted into the same Computer API actions an agent sends, and
travels the same queue.

Open the same URL in a second tab with `?token=dev` (read-only) to watch without
being able to operate, and press **Take control** in a `dev-write` tab to see the
lease change hands.

### Drive it from Python

```python
from iapetus import Iapetus                       # pip install ./sdk/python

client = Iapetus(api_key="sk_iap_live_demo")
print(client.viewer_url("dsk_1", user_id="you"))  # a URL a person opens to watch

with client.session("dsk_1", gateway_token="dev-write") as c:
    c.type("hello from the SDK")
    c.key("Enter")
    png = c.screenshot()                          # PNG bytes of the live screen
```

SDKs for [Python](sdk/python) and [TypeScript](sdk/typescript) — same surface.

The control plane is at <http://localhost:8090>. Mint a real token against it:

```bash
curl -s -X POST http://localhost:8090/v1/tokens \
  -H "Authorization: Bearer sk_iap_live_demo" \
  -H "Content-Type: application/json" \
  -d '{"type":"viewer","desktop_id":"dsk_1",
       "actor":{"type":"human","id":"you"},"control":"write"}'
```

## Without Docker

Docker is a convenience, not a requirement — it is the easiest way to get a
Linux desktop environment, nothing more.

- **Using the SDK** needs no Docker at all. It is pure Python/TypeScript over
  HTTP; point it at a desktop running anywhere — a remote host, a colleague's
  machine, the cloud.
- **The gateway and control plane** are cross-platform Rust binaries. `cargo run`
  starts them on macOS, Windows, or Linux with no container.
- **The desktop** (`iapetusd`) needs an X server and a browser, so it needs a
  Linux environment. On Linux that is native:

  ```bash
  sudo apt-get install -y xvfb openbox chromium
  scripts/run-native.sh          # builds and starts the whole stack, no Docker
  ```

  On macOS or Windows the desktop is the one part that still needs Linux, which
  Docker (or a remote Linux host) provides — the services and SDK run natively
  alongside it.

## What is in the box

Three services, one image, wired together by `docker-compose.yml`:

| Service | Crate | Role |
|---|---|---|
| `controlplane` | `iapetus-controlplane` | Issues, refreshes, and revokes Ed25519 JWTs; publishes the JWKS (§8.1) |
| `gateway` | `iapetus-gateway` | Relays the desktop's screen to viewers and their input back; holds the control lease (§6.1, §5.6) |
| `desktop` | `iapetusd` | The guest daemon inside one Desktop: capture, input, and the Computer API actions (§19.1) |

Two more crates are shared libraries, not services:

| Crate | Role |
|---|---|
| `iapetus-proto` | The wire types, the control-lease state machine, and the caps — the single source both sides compile from |
| `iapetus-auth` | Ed25519 JWT verification: the claim set and scope rules the issuer and verifier agree on |

## Architecture

```text
   browser (viewer)                          agent (Computer API)
        │                                            │
        │ WebSocket: screen ⇄ input                  │ gRPC over mTLS (§19.5)
        ▼                                            ▼
   ┌──────────┐   lease ⇄ input   ┌─────────────────────────────┐
   │ gateway  │◄─────────────────►│   desktop  (iapetusd)        │
   └──────────┘   screen tiles    │   ┌───────────────────────┐  │
        ▲                         │   │ capture · input · X11 │  │
        │ verifies JWTs           │   │ dispatch · lease      │  │
        ▼                         │   └───────────────────────┘  │
   ┌──────────────┐  JWKS         └─────────────────────────────┘
   │ controlplane │◄──── issues Viewer / Agent / Guest tokens
   └──────────────┘
```

The guest **dials out** to the gateway and the control plane; nothing connects
inward to a Desktop (§9.1). The gateway never decodes a pixel — it relays tile
bytes and caches the last one per position so a viewer can join mid-stream
(§19.6). Input from a viewer becomes the same Computer API actions an agent
sends, on one queue, so the lease can arbitrate between them (§5.6, §7.5).

## The Computer API

Every action in the specification is implemented (§7.2):

- **Observation** — `screenshot`, `window.list`, `screen.info`, `wait_for`
- **Input** — `mouse.move` / `click` / `down` / `up` / `drag`, `scroll`,
  `type`, `key` / `key.down` / `key.up`
- **Apps & shell** — `app.launch`, `app.install`, `shell.exec`, `secret.type`

`wait_for` settles on a stable screen, a window appearing, or a fixed duration;
`secret.type` types a stored credential without the plaintext reaching the
agent's context, the audit log, or a capture (§9.3).

## Building and testing

```bash
cargo test --workspace          # unit and integration tests, no display needed
cargo clippy --workspace --all-targets

# The L2 suite runs the real X11 drivers against a live X server in a container:
docker build -f images/linux-base/Dockerfile.test -t iapetus-l2 .
docker run --rm iapetus-l2
```

The L2 tests are the ones the specification calls the most important layer
(§15.2): what they miss — a click landing in the wrong place, Hangul arriving as
split jamo — surfaces only in production, so they are verified against a real X
server rather than mocked.

## Production vs. development

The compose stack is wired for development: the gateway trusts the shared
secrets `dev` (view) and `dev-write` (operate) so the quickstart needs no key
material. To verify real §8.1 JWTs instead, set `IAPETUS_JWKS` on the gateway to
the `kid:key` line the control plane prints at startup — then only tokens the
control plane signed are accepted, and the shared secrets stop working.

## Status

Implemented and tested end to end:

- The guest daemon: X11 capture (MIT-SHM) and input (XTEST), every Computer API
  action, verified against a live X server
- The stream gateway: the WebSocket JPEG-diff fallback path, ~1% idle CPU,
  full-screen change inside the §6.3 frame-rate budget
- The control lease (§5.6): human-preempts-agent, no queue between peers,
  idle handover, key release on handover
- Token issuance, refresh, revocation, and verification (§8.1)

Not yet built:

- Desktop provisioning (Firecracker microVMs) and the scheduler (§12.4, §19.2)
- The WebRTC media path — the default; today's stream is the §6.3 fallback
- The rest of the REST API: desktops, policy, webhooks, event stream (§8.4)
- Desktop provisioning, the WebRTC path, and the rest of the REST API above

## License

See [`Cargo.toml`](Cargo.toml).
