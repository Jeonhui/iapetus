/**
 * TypeScript SDK for Iapetus (PRD §8.6).
 *
 * The same two things a developer does with a Desktop as the Python SDK: hand a
 * person a URL to watch and operate it, and drive it with the Computer API.
 * Both go through the same services the rest of the system does — the control
 * plane issues tokens, the gateway relays the screen and forwards actions — so
 * this is a thin wrapper on `fetch`, with no dependencies.
 *
 * ```ts
 * import { Iapetus } from "iapetus-sdk";
 *
 * const client = new Iapetus({ apiKey: "sk_iap_live_..." });
 *
 * // Hand a person a URL to watch and take over (§7.5, §14.1).
 * console.log(await client.viewerUrl("dsk_1", { userId: "kim" }));
 *
 * // Drive it as an agent.
 * const c = await client.session("dsk_1", { gatewayToken: "dev-write" });
 * await c.launchApp({ key: "chrome", waitForWindow: true });
 * await c.type("iapetus");
 * await c.key("Enter");
 * const png = await c.screenshot();   // Uint8Array
 * ```
 */

/** An error returned by the control plane or the gateway, carrying the §8.9 code. */
export class IapetusError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(`${code}: ${message}`);
    this.name = "IapetusError";
    this.code = code;
  }
}

export type Control = "read" | "write";
export type Button = "left" | "middle" | "right";

export interface IapetusOptions {
  apiKey: string;
  /** Control plane base URL. Defaults to `http://localhost:8090`. */
  controlPlane?: string;
  /** Gateway base URL. Defaults to `http://localhost:8080`. */
  gateway?: string;
}

async function post(url: string, body: unknown, headers?: Record<string, string>): Promise<any> {
  const resp = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...(headers ?? {}) },
    body: JSON.stringify(body),
    // An action waits on the guest; give it room before fetch's own timeout.
    signal: AbortSignal.timeout(35_000),
  });
  const text = await resp.text();
  if (!resp.ok) {
    try {
      const payload = JSON.parse(text);
      throw new IapetusError(payload.code ?? String(resp.status), payload.message ?? text);
    } catch (e) {
      if (e instanceof IapetusError) throw e;
      throw new IapetusError(String(resp.status), text);
    }
  }
  return text ? JSON.parse(text) : {};
}

/**
 * A client scoped to one project, holding the Project Key (§8.1).
 *
 * The Project Key never leaves the server that holds this client — it mints the
 * short-lived tokens handed to browsers and agents. Do not construct this in a
 * browser or pass the key to an agent process.
 */
export class Iapetus {
  private readonly apiKey: string;
  private readonly controlPlane: string;
  private readonly gateway: string;

  constructor(opts: IapetusOptions) {
    this.apiKey = opts.apiKey;
    this.controlPlane = (opts.controlPlane ?? "http://localhost:8090").replace(/\/$/, "");
    this.gateway = (opts.gateway ?? "http://localhost:8080").replace(/\/$/, "");
  }

  // ── Tokens ──────────────────────────────────────────────

  /**
   * Mints a Viewer Token for a person (§8.1). `control` is the *maximum* level
   * the token may request, not the lease itself — a viewer starts observing and
   * takes the lease only when the person presses the button (§7.5).
   */
  async issueViewerToken(desktopId: string, userId: string, control: Control = "write"): Promise<string> {
    return this.issue("viewer", desktopId, userId, "human", { control });
  }

  /** Mints an Agent Token scoped to one Desktop (§8.1). */
  async issueAgentToken(desktopId: string, agentId: string, scopes?: string[]): Promise<string> {
    return this.issue("agent", desktopId, agentId, "agent", {
      scopes: scopes ?? ["desktop:control", "desktop:shell", "desktop:files"],
    });
  }

  private async issue(
    kind: string,
    desktopId: string,
    actorId: string,
    actorType: string,
    extra: { control?: Control; scopes?: string[] },
  ): Promise<string> {
    const body: Record<string, unknown> = {
      type: kind,
      desktop_id: desktopId,
      actor: { type: actorType, id: actorId },
      control: extra.control ?? "read",
    };
    if (extra.scopes) body.scopes = extra.scopes;
    const resp = await post(`${this.controlPlane}/v1/tokens`, body, {
      Authorization: `Bearer ${this.apiKey}`,
    });
    return resp.token as string;
  }

  // ── Viewer ──────────────────────────────────────────────

  /**
   * A complete URL a person opens to watch and operate the Desktop. The token
   * is embedded, so this is a whole address, not a resource to fetch (§8.1).
   * Print it before the agent starts, so the window is open before anything
   * moves (§14.1).
   */
  async viewerUrl(desktopId: string, opts: { userId: string; control?: Control }): Promise<string> {
    const token = await this.issueViewerToken(desktopId, opts.userId, opts.control ?? "write");
    return `${this.gateway}/?token=${token}`;
  }

  // ── Driving ─────────────────────────────────────────────

  /**
   * Opens a session to drive the Desktop with the Computer API. Without
   * `gatewayToken` the client mints an Agent Token itself; pass one (for
   * instance the development `"dev-write"`) to use a gateway not verifying JWTs.
   */
  async session(desktopId: string, opts?: { gatewayToken?: string }): Promise<Session> {
    const token = opts?.gatewayToken ?? (await this.issueAgentToken(desktopId, "sdk-agent"));
    return new Session(this.gateway, token);
  }
}

/**
 * The Computer API against one Desktop (§7.2). Each method sends one action and,
 * for the ones that return something, awaits the guest's reply. Actions on one
 * session execute in arrival order, so a screenshot after a click reflects it
 * (§6.3).
 */
export class Session {
  private readonly gateway: string;
  private readonly token: string;

  constructor(gateway: string, token: string) {
    this.gateway = gateway;
    this.token = token;
  }

  private async act(action: Record<string, unknown>): Promise<any> {
    const resp = await post(`${this.gateway}/v1/action?token=${this.token}`, action);
    if (!resp.ok) throw new IapetusError(resp.error ?? "EXEC_FAILED", resp.message ?? "");
    return resp;
  }

  async click(x: number, y: number, button: Button = "left"): Promise<void> {
    await this.act({ type: "mouse.move", x, y });
    await this.act({ type: "mouse.down", button });
    await this.act({ type: "mouse.up", button });
  }

  async move(x: number, y: number): Promise<void> {
    await this.act({ type: "mouse.move", x, y });
  }

  async scroll(x: number, y: number, dx = 0, dy = 0): Promise<void> {
    await this.act({ type: "scroll", x, y, dx, dy });
  }

  /** Types text through the IME, so Hangul arrives whole, not as jamo (§15.2). */
  async type(text: string): Promise<void> {
    await this.act({ type: "type", text });
  }

  /** Presses a key or chord, e.g. `"Enter"` or `"ctrl+c"`. */
  async key(keys: string): Promise<void> {
    await this.act({ type: "key", keys });
  }

  /** Launches a catalog app by key, or any program by path (§5.5, §7.3). */
  async launchApp(opts: { key?: string; command?: string; waitForWindow?: boolean }): Promise<any> {
    return this.act({
      type: "app.launch",
      key: opts.key,
      command: opts.command,
      wait_for_window: opts.waitForWindow ?? false,
    });
  }

  /**
   * Captures the screen and returns PNG bytes. A screenshot always postdates
   * the action before it (§6.3), so it is safe to read the result of a click by
   * taking one straight after.
   */
  async screenshot(): Promise<Uint8Array> {
    const resp = await this.act({ type: "screenshot" });
    if (typeof resp.screenshot !== "string") {
      throw new IapetusError("EXEC_FAILED", "the screenshot carried no image");
    }
    // Base64 → bytes, using whichever primitive the runtime has.
    if (typeof Buffer !== "undefined") {
      return new Uint8Array(Buffer.from(resp.screenshot, "base64"));
    }
    const binary = atob(resp.screenshot);
    return Uint8Array.from(binary, (ch) => ch.charCodeAt(0));
  }
}
