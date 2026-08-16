# Parent-app demo

A minimal parent product that embeds an Iapetus desktop and drives it — the
integration a customer builds (§7.5 V-09, §14.1).

The Project Key lives on this app's server, never the browser (§8.1). The server
mints a short-lived viewer token, the page embeds the desktop in an iframe, and
agent actions are proxied through the server.

## Run

```bash
docker compose up --build          # in the repo root — the desktop, gateway, control plane
node examples/parent-app/server.mjs
```

Open <http://localhost:3000>. The desktop is embedded on the right; the buttons
on the left drive the agent, and you can also click into the desktop and type
yourself — you and the agent share one screen and one control lease (§5.6).

Framing works because the compose gateway is started with
`IAPETUS_EMBED_ORIGINS=http://localhost:3000`; without a registered origin the
viewer refuses to be framed (`frame-ancestors 'none'`), which is the safe
default for a token-bearing page.
