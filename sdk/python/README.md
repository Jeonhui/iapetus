# Iapetus Python SDK

A thin, dependency-free client for [Iapetus](../../README.md) — a persistent
virtual desktop an agent and a human co-own.

Two things a developer does with a Desktop: hand a person a URL to watch and
operate it, and drive it with the Computer API. Both go through the same
services the rest of the system does, so this is a wrapper, not a second
implementation.

## Install

```bash
pip install ./sdk/python
```

No runtime dependencies — the SDK uses only the standard library.

## Use

Start the stack (`docker compose up --build`), then:

```python
from iapetus import Iapetus

client = Iapetus(api_key="sk_iap_live_demo")

# Hand a person a URL to watch and take over (§7.5, §14.1).
print(client.viewer_url("dsk_1", user_id="kim"))

# Drive it as an agent.
with client.session("dsk_1", gateway_token="dev-write") as c:
    c.launch_app("chrome", wait_for_window=True)
    c.type("iapetus")
    c.key("Enter")
    png = c.screenshot()          # PNG bytes
```

`gateway_token="dev-write"` uses the compose stack's development shared secret.
Against a gateway verifying real §8.1 JWTs, drop it and the client mints an
Agent Token from the control plane itself.

## Surface

| Call | Does |
|---|---|
| `viewer_url(desktop, user_id)` | A complete URL a person opens to watch and operate |
| `issue_viewer_token` / `issue_agent_token` | Mint a short-lived token from the control plane |
| `session(desktop)` | Open the Computer API |
| `c.click / move / scroll` | Pointer input |
| `c.type / key` | Keyboard input (Hangul-safe via the IME) |
| `c.launch_app` | Launch a catalog app or any program by path |
| `c.screenshot()` | Capture the screen as PNG bytes |

Errors carry the §8.9 code: `except IapetusError as e: e.code`.
