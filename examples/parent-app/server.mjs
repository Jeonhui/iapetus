// A parent product that embeds an Iapetus desktop and drives it.
//
// This is the integration a customer builds: the Project Key lives on the
// server (never the browser, §8.1), the server mints short-lived tokens and
// proxies agent actions, and the page embeds the viewer in an iframe (§7.5
// V-09). The browser never sees the Project Key.
//
//   docker compose up --build          # in the repo root
//   node examples/parent-app/server.mjs
//   open http://localhost:3000
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { Iapetus } from "../../sdk/typescript/src/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
const DESKTOP = "dsk_1";

// The client holds the Project Key. It stays on this server.
const iapetus = new Iapetus({ apiKey: "sk_iap_live_demo" });

async function body(req) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  return chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
}

function json(res, status, obj) {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(obj));
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");

    // The page itself.
    if (req.method === "GET" && url.pathname === "/") {
      const html = await readFile(join(here, "index.html"), "utf8");
      res.writeHead(200, { "Content-Type": "text/html" });
      return res.end(html);
    }

    // A viewer URL for the person, minted server-side so the token is
    // short-lived and the Project Key never leaves this process (§8.1, §14.1).
    if (req.method === "GET" && url.pathname === "/api/viewer-url") {
      const viewerUrl = await iapetus.viewerUrl(DESKTOP, { userId: "demo-user" });
      return json(res, 200, { url: viewerUrl });
    }

    // Agent actions, proxied. The customer's own auth would gate this; here it
    // is open for the demo. The gateway's dev shared secret drives the desktop.
    if (req.method === "POST" && url.pathname === "/api/agent") {
      const { action, arg } = await body(req);
      const c = await iapetus.session(DESKTOP, { gatewayToken: "dev-write" });
      let out = {};
      if (action === "launch") await c.launchApp({ command: "/usr/bin/chromium", waitForWindow: true });
      else if (action === "type") await c.type(arg ?? "");
      else if (action === "enter") await c.key("Return");
      else if (action === "screenshot") {
        const png = await c.screenshot();
        out.png = Buffer.from(png).toString("base64");
      } else return json(res, 400, { error: "unknown action" });
      return json(res, 200, { ok: true, ...out });
    }

    res.writeHead(404);
    res.end("not found");
  } catch (e) {
    json(res, 500, { error: String(e?.message ?? e), code: e?.code });
  }
});

server.listen(3000, () => console.log("parent app on http://localhost:3000"));
