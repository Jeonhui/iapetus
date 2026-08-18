# Iapetus SDK for JavaScript & TypeScript

A thin, dependency-free client for [Iapetus](../../README.md) — a persistent
virtual desktop an agent and a human co-own. Uses only the runtime's built-in
`fetch`, so it runs on Node 18+ and in the browser with nothing to install.

**One package, both languages.** It is written in TypeScript and ships compiled
JavaScript plus type declarations, so JavaScript users `import` it directly and
TypeScript users also get full types — no separate JS build to maintain.

## Install

```bash
npm install iapetus-sdk       # or: npm install ./sdk/typescript
```

## Use

Start the stack (`docker compose up --build`), then:

```ts
import { Iapetus } from "iapetus-sdk";

const client = new Iapetus({ apiKey: "sk_iap_live_demo" });

// Hand a person a URL to watch and take over (§7.5, §14.1).
console.log(await client.viewerUrl("dsk_1", { userId: "kim" }));

// Drive it as an agent.
const c = await client.session("dsk_1", { gatewayToken: "dev-write" });
await c.launchApp({ key: "chrome", waitForWindow: true });
await c.type("iapetus");
await c.key("Enter");
const png = await c.screenshot();   // Uint8Array
```

`gatewayToken: "dev-write"` uses the compose stack's development shared secret.
Against a gateway verifying real §8.1 JWTs, drop it and the client mints an
Agent Token from the control plane itself.

## Surface

Mirrors the [Python SDK](../python): `viewerUrl`, `issueViewerToken` /
`issueAgentToken`, and `session()` returning the Computer API — `click`, `move`,
`scroll`, `type`, `key`, `launchApp`, `screenshot`. Errors carry the §8.9 code:
`catch (e) { if (e instanceof IapetusError) e.code }`.
