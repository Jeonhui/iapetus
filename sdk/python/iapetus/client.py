"""Python SDK for Iapetus (PRD §8.6).

The whole surface is two things a developer does with a Desktop: hand a person a
URL to watch and operate it, and drive it with the Computer API. Both go through
the same services the rest of the system does — the control plane issues tokens,
the gateway relays the screen and forwards actions — so the SDK is a thin,
dependency-free wrapper, not a second implementation of anything.

    from iapetus import Iapetus

    client = Iapetus(api_key="sk_iap_live_...")

    # Hand a person a URL to watch and take over (§7.5, §14.1).
    print(client.viewer_url("dsk_1", user_id="kim"))

    # Drive it as an agent.
    with client.session("dsk_1") as c:
        c.launch_app("chrome", wait_for_window=True)
        c.type("iapetus")
        c.key("Enter")
        png = c.screenshot()

Only the standard library is used, so the SDK adds nothing to an agent's
environment.
"""

from __future__ import annotations

import base64
import json
import urllib.error
import urllib.request
from typing import Any, Optional

__all__ = ["Iapetus", "Session", "IapetusError"]


class IapetusError(RuntimeError):
    """An error returned by the control plane or the gateway.

    Carries the machine-readable ``code`` from §8.9 alongside the message, so a
    caller can branch on ``CONTROL_LOST`` or ``ACTION_TIMEOUT`` rather than
    parsing a string.
    """

    def __init__(self, code: str, message: str):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


def _post(url: str, body: dict, headers: Optional[dict] = None) -> dict:
    data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=35) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            payload = json.loads(raw)
            raise IapetusError(payload.get("code", str(e.code)), payload.get("message", raw.decode()))
        except (json.JSONDecodeError, AttributeError):
            raise IapetusError(str(e.code), raw.decode(errors="replace")) from None


class Iapetus:
    """A client scoped to one project, holding the Project Key (§8.1).

    The Project Key never leaves the server that holds this client — it is used
    only to mint the short-lived tokens that are handed to browsers and agents,
    exactly as §8.1 requires. Do not construct this in a browser or pass the key
    to an agent process.
    """

    def __init__(
        self,
        api_key: str,
        control_plane: str = "http://localhost:8090",
        gateway: str = "http://localhost:8080",
    ):
        self.api_key = api_key
        self.control_plane = control_plane.rstrip("/")
        self.gateway = gateway.rstrip("/")

    # ── Tokens ────────────────────────────────────────────────

    def issue_viewer_token(self, desktop_id: str, user_id: str, control: str = "write") -> str:
        """Mints a Viewer Token for a person (§8.1).

        ``control`` is the *maximum* level the token may request, not the lease
        itself — a viewer always starts observing and takes the lease only when
        the person presses the button (§7.5).
        """
        return self._issue("viewer", desktop_id, user_id, "human", control=control)

    def issue_agent_token(self, desktop_id: str, agent_id: str, scopes: Optional[list] = None) -> str:
        """Mints an Agent Token scoped to one Desktop (§8.1)."""
        return self._issue(
            "agent", desktop_id, agent_id, "agent",
            scopes=scopes or ["desktop:control", "desktop:shell", "desktop:files"],
        )

    def _issue(self, kind, desktop_id, actor_id, actor_type, control="read", scopes=None) -> str:
        body: dict[str, Any] = {
            "type": kind,
            "desktop_id": desktop_id,
            "actor": {"type": actor_type, "id": actor_id},
            "control": control,
        }
        if scopes is not None:
            body["scopes"] = scopes
        resp = _post(
            f"{self.control_plane}/v1/tokens",
            body,
            headers={"Authorization": f"Bearer {self.api_key}"},
        )
        return resp["token"]

    # ── Viewer ────────────────────────────────────────────────

    def viewer_url(self, desktop_id: str, user_id: str, control: str = "write") -> str:
        """A complete URL a person opens to watch and operate the Desktop.

        The token is embedded, so this is a whole address rather than a resource
        to fetch (§8.1). Print it before the agent starts working, so the person
        has the window open before anything moves (§14.1).
        """
        token = self.issue_viewer_token(desktop_id, user_id, control)
        return f"{self.gateway}/?token={token}"

    # ── Driving ───────────────────────────────────────────────

    def session(self, desktop_id: str, gateway_token: Optional[str] = None) -> "Session":
        """Opens a session to drive the Desktop with the Computer API.

        Without ``gateway_token`` the client mints an Agent Token itself; pass
        one (for instance the development ``"dev-write"``) to use a gateway that
        is not verifying real JWTs.
        """
        token = gateway_token or self.issue_agent_token(desktop_id, "sdk-agent")
        return Session(self.gateway, desktop_id, token)


class Session:
    """The Computer API against one Desktop (§7.2).

    Each method sends one action and, for the ones that return something, waits
    for the guest's reply. Actions on one session execute in arrival order, so a
    screenshot after a click reflects the click (§6.3).
    """

    def __init__(self, gateway: str, desktop_id: str, token: str):
        self._gateway = gateway
        self._desktop_id = desktop_id
        self._token = token

    def __enter__(self) -> "Session":
        return self

    def __exit__(self, *exc) -> None:
        return None

    def _act(self, **action) -> dict:
        resp = _post(f"{self._gateway}/v1/action?token={self._token}", action)
        if not resp.get("ok", False):
            raise IapetusError(resp.get("error", "EXEC_FAILED"), resp.get("message", ""))
        return resp

    # Input — fire and confirm, no payload back.
    def click(self, x: int, y: int, button: str = "left") -> None:
        self._act(type="mouse.move", x=x, y=y)
        self._act(type="mouse.down", button=button)
        self._act(type="mouse.up", button=button)

    def move(self, x: int, y: int) -> None:
        self._act(type="mouse.move", x=x, y=y)

    def scroll(self, x: int, y: int, dx: int = 0, dy: int = 0) -> None:
        self._act(type="scroll", x=x, y=y, dx=dx, dy=dy)

    def type(self, text: str) -> None:
        """Types text through the IME, so Hangul arrives whole, not as jamo (§15.2)."""
        self._act(type="type", text=text)

    def key(self, keys: str) -> None:
        """Presses a key or chord, e.g. ``"Enter"`` or ``"ctrl+c"``."""
        self._act(type="key", keys=keys)

    def launch_app(self, key: Optional[str] = None, command: Optional[str] = None,
                   wait_for_window: bool = False) -> dict:
        """Launches a catalog app by key, or any program by path (§5.5, §7.3)."""
        action: dict[str, Any] = {"type": "app.launch", "wait_for_window": wait_for_window}
        if key:
            action["key"] = key
        if command:
            action["command"] = command
        return self._act(**action)

    def screenshot(self) -> bytes:
        """Captures the screen and returns PNG bytes.

        A screenshot always postdates the action before it (§6.3), so it is safe
        to read the result of a click by taking one straight after.
        """
        resp = self._act(type="screenshot")
        b64 = resp.get("screenshot")
        if b64 is None:
            raise IapetusError("EXEC_FAILED", "the screenshot carried no image")
        return base64.b64decode(b64)
