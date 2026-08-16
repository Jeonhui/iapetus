// Drive a Desktop, and hand a human a URL to watch it.
//
//   docker compose up --build
//   node --experimental-strip-types sdk/typescript/examples/quickstart.ts
import { Iapetus } from "../src/index.ts";

const client = new Iapetus({ apiKey: "sk_iap_live_demo" });

// Print the viewer URL before anything moves, so a person can open it (§14.1).
console.log("Watch here:", await client.viewerUrl("dsk_1", { userId: "you" }));

// The compose gateway runs in development mode, trusting the shared secret
// "dev-write"; a production gateway verifies a real Agent Token and this is
// dropped.
const c = await client.session("dsk_1", { gatewayToken: "dev-write" });
await c.type("hello from the TypeScript SDK");
await c.key("Enter");
const png = await c.screenshot();
const { writeFile } = await import("node:fs/promises");
await writeFile("desktop.png", png);
console.log(`Wrote desktop.png (${png.length} bytes)`);
