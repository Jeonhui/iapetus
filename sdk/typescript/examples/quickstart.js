// Plain JavaScript — no TypeScript, no build. The published package ships
// compiled JS, so `import` works directly in Node 18+ and the browser.
//
//   npm install iapetus-sdk
//   node examples/quickstart.js
import { Iapetus } from "iapetus-sdk";

const client = new Iapetus({ apiKey: "sk_iap_live_demo" });

// Hand a person a URL to watch and take over.
console.log("Watch here:", await client.viewerUrl("dsk_1", { userId: "you" }));

// Drive it as an agent.
const c = await client.session("dsk_1", { gatewayToken: "dev-write" });
await c.type("hello from JavaScript");
await c.key("Enter");
const png = await c.screenshot();
const { writeFile } = await import("node:fs/promises");
await writeFile("desktop.png", png);
console.log(`Wrote desktop.png (${png.length} bytes)`);
