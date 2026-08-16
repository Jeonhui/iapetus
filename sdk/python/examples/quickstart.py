"""Drive a Desktop, and hand a human a URL to watch it.

Run the stack first:

    docker compose up --build

Then:

    python examples/quickstart.py
"""
from iapetus import Iapetus

client = Iapetus(api_key="sk_iap_live_demo")

# Print the viewer URL before anything moves, so a person can open it and watch
# (§14.1). The Desktop id is fixed here because the compose stack runs one.
print("Watch here:", client.viewer_url("dsk_1", user_id="you"))

# The compose gateway runs in development mode, trusting the shared secret
# "dev-write"; a production gateway verifies a real Agent Token and this
# argument is dropped.
with client.session("dsk_1", gateway_token="dev-write") as c:
    c.type("hello from the SDK")
    c.key("Enter")
    png = c.screenshot()
    with open("desktop.png", "wb") as f:
        f.write(png)
    print(f"Wrote desktop.png ({len(png)} bytes)")
