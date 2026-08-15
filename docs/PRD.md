# Iapetus — Agent Virtual Desktop Platform: Product Specification

> **A persistent virtual desktop co-owned by an AI agent and a human.**
> The agent connects over an API, the person connects through a browser, and both operate the same machine with equal authority.

| Field | Value |
|---|---|
| Version | **v0.9.2** |
| Last updated | 2026-08-15 |
| Status | Draft — before technical validation |
| Document owner | Product (Jeonhui Lee) |
| Language | English (Korean original: `docs/PRD.ko.md`) |
| Required reviewers | Engineering lead, Security, Legal (§10, §18), Finance (§13) |
| Related documents | (planned) API reference / image build guide / DPA template |
| Repository | `Jeonhui/iapetus` |

### Revision history

| Version | Date | Principal changes |
|---|---|---|
| v0.1 | 2026-08-15 | Initial draft. Desktop, Computer API, OS abstraction, roadmap |
| v0.1.1 | 2026-08-15 | Authority model inverted: app allowlist-by-default → agent OWNER (root) by default. Stack fixed (Rust + OCI image + microVM) |
| v0.2 | 2026-08-15 | **Co-ownership model introduced** (humans and agents hold equal authority, share one OS session, control lease arbitration). Screen pipeline fixed on WebRTC. New sections for authn/authz, policy/secret/webhook APIs, compliance and data lifecycle, operations/SLA/runbooks, onboarding, test strategy, competitive analysis, risk register. Full sweep of internal contradictions and cross-references |
| v0.3 | 2026-08-15 | **Design review applied.** Media topology fixed (guest is an SFU source, P2P rejected, multi-viewer handled by temporal layers). Runtime reselected (Kata → Firecracker, no snapshot API). Snapshot restore constraints stated (CPU template, host affinity, clock resync, dead sockets). Writing to a SUSPENDED volume now invalidates the snapshot. crypto-shredding corrected from T+0 to T+24h. Egress default tiered, DoH addressed. Token self-refresh with a total-lifetime cap. Control lease always requires an explicit acquire — opening a viewer never preempts. Screenshot freshness contract. Codec raised to High Profile with a lossless still overlay. Capacity figures consolidated into §12.4 as the single source. Glossary aligned to v0.2 |
| v0.4 | 2026-08-15 | **Second review applied; open decisions closed.** Encoder x264 → **OpenH264** (x264 cannot emit temporal layers) with capacity re-derived (concurrent observation 8 → 6). Masking moved from the gateway to the **guest Frame Source, before encoding**. Still overlay quantified (changed region, WebP q95, 200KB cap, DataChannel). Latency budget gained a row for layer-downgraded viewers and the KPI was scoped accordingly. DEGRADED included in the SLA denominator. **Every open issue decided** (Azure Windows procurement, three third-party app principles, end-user protections, `desktop_type` split). Named commercial applications generalized throughout |
| v0.5 | 2026-08-15 | **Third review applied.** End-user protection moved from contractual to **technical** — `desktop:owners:manage` and `desktop:audit:read` are granted unconditionally to human tokens and cannot be withheld (`CANNOT_WAIVE_HUMAN_RIGHTS`). Encoder change propagated into the latency budget (12–30ms, total 57–199ms) with the admission that **software encoding leaves no KPI headroom**, hence a GPU tier for latency-sensitive use. OpenH264 parameter names corrected, the overlay table's "lossless / identical for all viewers" claim withdrawn, DataChannel bandwidth contention stated, R-06 aligned to the procurement decision |
| v0.6 | 2026-08-15 | **Fourth review applied.** Restored the **agent provisioning path** that narrowing `project:manage` had severed — `POST /v1/desktops` accepts `owners[]` at creation only, while post-creation changes stay human-only. Clarified that what is protected is *removal of a human owner*, not first assignment. Recorded in §9.3 that this layer stops scope stripping but not identity forgery (tracked as R-07). Corrected the L0 latency lower bound (157 → 57ms). Rebased SLA credits on shortfall against each target, since a Windows desktop meeting 99.0% would otherwise have earned a credit |
| v0.7 | 2026-08-15 | **Interface specification filled in (implementation-readiness audit).** Review to date had only attacked what was arguable; the uncontested enumeration work sat empty. Added §8.2 API conventions (ULID identifiers, RFC 3339, integer coordinates, error envelope, cursor pagination, eleven caps, timeouts, encoding, versioning), §8.3 resource schemas (every Desktop field plus `spec_tier` tiering; Image/Policy/Secret/Webhook; Snapshot and Organization cut from v1), §8.4 async job model with control lease, owner, and session bodies, §8.7 event envelope with per-type payloads and SSE reconnection, §19.5 `iapetusd` transport contract (gRPC/Protobuf, vsock health probe, no retransmission), §19.6 guest↔gateway media contract, and the §7.5 viewer input path. New `DELETING` state. Idempotency keys made mandatory. Six error codes added |
| v0.8 | 2026-08-15 | **Propagation misses corrected (re-audit).** v0.7 had written its fixes as *new standalone sections*, leaving the old normative text in place — resolved by adding a **precedence rule** in §0 and aligning the §5 examples (`spec` → `spec_tier`, `bounds` as an object), the §8.4 webhook wire format, the §9.4 audit record, and the §10.3 deletion flow to §8.2/§8.3. **§12.4 re-derived for four tiers** (per-tier host pools, `light` capped by CPU, per-desktop encoding reservation) — the one part that had regressed in v0.7. Idempotency key contract specified (format and `(project, session, endpoint)` scope) with the WebSocket and viewer paths explicitly exempt. Phase 1 health probe branched per runtime (Docker `exec` / vsock / hvsocket). Image `source` added (registry only in v1), `PUT`/`DELETE .../policy` added, `viewer_url` defined. Boundary between this document and the API reference stated |
| **v0.9** | 2026-08-15 | **Density and authority re-examined; auth completed.** §6.4 gained the **rejection of multi-session** (isolation ladder, break-even at 28% activity ratio) and the **deferral of Desktop Group** (2.3× better for same-trust-domain parallelism, designed so the API model is unchanged, v2). §8.1 gained a token signing scheme — Ed25519, JWKS, 90-day rotation, the `jti` claim that revocation had been operating on without ever defining, and `orig_iat` for total-lifetime enforcement. §12.5 added **six lightweighting levers**. §12.4 corrected `light` from 40 to **34** (the 2.5:1 ceiling includes the encoding reservation) and three stale `28`s removed. Precedence rule gained a **single-source-per-topic** layer. Control lease arbitration gained a row for agent-versus-agent contention |
| v0.9.1 | 2026-08-15 | Document translated to English. No design changes; terminology fixed against a locked glossary and verified mechanically. The Korean original is retained at `docs/PRD.ko.md` |
| **v0.9.2** | 2026-08-15 | **Embedding contract added (parent-product integration audit).** V-09 had committed the viewer to iframe embedding without a single rule making it safe or workable: no frame policy, no origin allowlist, and no way for the host page to learn viewer-local state. §7.5 gained **Embedding in a parent product** — framing denied by default, per-project `embed_origins` (exact match, no wildcard subdomains), a second in-page origin check because CSP fails open where it is not enforced, and a versioned `postMessage` contract. The `token_expiring` message closes a real hole: an embedded viewer would otherwise go black at the §8.1 eight-hour refresh cap with no signal to the host. §9.2 gained the matching policy row and §8.3 the matching schema field, project-level only. A blank line that had split the revision table in two was removed |

### How to read this

| Role | Start here |
|---|---|
| Decision maker | §1 Overview → §4 Competitive analysis → §17 Risks → §18 Open issues |
| Agent developer (our user) | §3 Scenarios → §7 Functional spec → §8 Interfaces → §14 Onboarding |
| Implementation engineer | §5 Data model → §6 Architecture → §19 Stack → §15 Testing |
| Security and legal | §9 Security → §10 Compliance → §17 Risks |

### Precedence

When the same fact appears in more than one place, **the order below decides.** Without this rule every revision leaves stale text behind and the document ends up asserting two different truths.

| Rank | Source | Nature |
|---|---|---|
| 1 | **§8.2 API conventions · §8.3 resource schemas** | The only normative source for wire format |
| 2 | **§19.5 · §19.6 transport contracts** | The only normative source for internal protocols |
| 3 | Explicit decisions elsewhere in the body | |
| 4 | JSON examples in §5 and similar | **Illustrative, never normative.** If a field's shape disagrees, §8.3 is right |

Examples exist to convey a concept. They are not cited as evidence for a field's name, type, or format.

**When two sources of equal rank disagree — single source per topic.** The table above separates *normative from illustrative* only. A number or decision that appears in several body sections (all rank 3) cannot be adjudicated that way, so **each topic has exactly one section that owns it.**

| Topic | Owner |
|---|---|
| Capacity and placement density | §12.4 |
| Latency budget and media parameters | §6.3 |
| Caps, timeouts, identifiers | §8.2 |
| State transitions | §5.4 |
| Authority and scopes | §8.1 |

When another section refers to these values, it **cites the section rather than copying the number.** A copied number survives the change to its original.

---

## 0. Table of contents

- [1. Overview](#1-overview) — product definition and scope
- [2. Goals and Success Metrics](#2-goals-and-success-metrics) — KPIs
- [3. Users and Scenarios](#3-users-and-scenarios) — personas and scenarios S1–S6
- [4. Competitive Analysis](#4-competitive-analysis) — position against alternatives, and where we lose
- [5. Core Concepts and Data Model](#5-core-concepts-and-data-model) — Desktop, Owner, Session
- [6. System Architecture](#6-system-architecture) — control/data plane and the screen pipeline
- [7. Functional Specification](#7-functional-specification) — Computer API and the authority model
- [8. External Interface Specification](#8-external-interface-specification) — auth, REST, WS, SDK, MCP
- [9. Security and Policy](#9-security-and-policy) — isolation, policy, audit
- [10. Compliance and Data Lifecycle](#10-compliance-and-data-lifecycle) — personal data, deletion, certification
- [11. Non-Functional Requirements](#11-non-functional-requirements) — performance and scale requirements
- [12. Operations, SLA, and Incident Response](#12-operations-sla-and-incident-response) — SLA, runbooks, capacity
- [13. Pricing Model](#13-pricing-model) — billable items
- [14. Onboarding and User Journey](#14-onboarding-and-user-journey) — the first fifteen minutes
- [15. Test and Verification Strategy](#15-test-and-verification-strategy) — test layers and acceptance criteria
- [16. Roadmap](#16-roadmap) — Phases 1–4
- [17. Risk Register](#17-risk-register) — risks, hypotheses, kill criteria
- [18. Open Issues](#18-open-issues) — what remains
- [19. Technology Stack](#19-technology-stack) — Rust, Docker, microVM
- [20. Glossary](#20-glossary) — terms

---

## 1. Overview

### 1.1 In one line

**Iapetus provides a persistent virtual desktop — a Computer — that an AI agent and a human own and operate together.**

The agent lives outside Iapetus (in a customer product, an agent runtime, a workflow engine). Iapetus supplies only the **computer resource** the agent attaches to in order to see the screen, click, type, and launch applications.

**A Desktop is co-owned by a human and an agent. Their authority is identical.**

- **Agent**: root / Administrator. Runs, installs, and removes arbitrary programs; full filesystem; system settings.
- **Human (owner)**: the same. Connects through a browser and uses it like their own PC. Anything the agent can do, they can do.

The agent is **not a reduced-privilege delegate but an equal one**. The two **share one OS session** — if the agent signs into a messenger, the human sees that same signed-in state, and vice versa.

Constraints are placed at the Desktop **boundary**, not **inside** it (§9).

### 1.2 Problem

Where today's agent automation tooling stops:

| Approach | Limitation |
|---|---|
| API / MCP integration | Applications without an API (internal ERP, desktop messengers, Excel macros) cannot be automated |
| Browser automation (Playwright and similar) | Web only. No desktop applications |
| Driving the user's own PC | Occupies their machine. No parallelism, no unattended operation, no isolation |
| Single-use sandboxes | Logins, installed applications, and files disappear when the session ends |

What is missing is **a computer that belongs to the agent, does not die, and stays signed in.**

### 1.3 Approach

Give the agent a **persistent desktop** as a first-class resource.

```text
Agent = Brain(LLM) + Memory + Tools + Routines + Computer(Iapetus)
```

- The Desktop stays alive after the user closes the browser.
- Messenger logins, Chrome cookies, and downloaded files remain.
- The next run reconnects to the same Desktop and continues.

### 1.4 Scope

**In scope**
- Virtual desktop provisioning and lifecycle
- A unified Computer API (screenshot / click / type / key / scroll / launch_app, …)
- OS abstraction (Linux, Windows)
- An application catalog plus arbitrary program execution and installation, with administrator rights
- Persistent storage and session-state preservation
- **A full-access viewer for humans** (observe, operate fully, transfer files)
- Control lease arbitration between human and agent
- REST/WebSocket APIs, SDKs, and an MCP server for external agents

**Out of scope (v1)**
- Providing an LLM or inference engine — the agent lives outside
- Agent prompting or planning frameworks
- Element-level semantic accessibility-tree control → considered for v2
- Mobile OS (Android/iOS) desktops → considered for v2

---

## 2. Goals and Success Metrics

### 2.1 Product goals

1. An agent operates the computer through one API **without knowing which OS it is.**
2. An agent holds **the same complete authority a human has** over that Desktop. There is no "you cannot open that application."
3. A human can **connect directly to the same Desktop with the same authority** and use it like their own PC.
4. The two share **one OS session and one set of logins.**
5. A Desktop persists **for weeks and months, not minutes.**
6. One account can **run tens to hundreds of Desktops in parallel.**

### 2.2 Success metrics (KPI)

| Metric | Target | Condition |
|---|---|---|
| Cold start (create → ACTIVE) | Linux < 15s, Windows < 60s | Without a warm pool |
| Warm start (SUSPENDED → ACTIVE) | < 5s | **Local NVMe snapshots only.** Network storage < 20s |
| Instantaneous action round trip, p95 (`click`, `key`, `scroll`) | < 300ms | Same region |
| Sustained actions (`type`, `mouse.drag`) | Overhead < 150ms | Measured as pure overhead, excluding input duration |
| `screenshot` p95 (1080p, JPEG q80) | < 500ms | Includes satisfying the freshness contract (§6.3); not a cached frame |
| Streaming frame latency (glass-to-glass p95) | < 200ms | **Same region, full-layer viewers only.** The software encoding budget of 57–199ms leaves almost no headroom; the GPU tier reaches ~179ms (§6.3) |
| Layer downgrade rate | < 5% | Downgraded viewers fall outside the latency KPI above, so the downgrade itself is the managed metric |
| **Login session preservation** (no re-authentication after suspend/resume) | **> 99.9%** | One failure in a hundred destroys trust in the product, so 99% is not sufficient |
| Control lease preemption latency | < 500ms | From the human pressing the button to the agent receiving `CONTROL_LOST` (§5.6) |
| Monthly availability | Linux 99.5% / Windows 99.0% | Different infrastructure, so promised separately (§12.1) |
| Time to first action (TTFA, median) | < 15 min | Signup to first successful action. Measured from Phase 2 (§14.5) |

---

## 3. Users and Scenarios

### 3.1 Primary users

| Type | Description | Need |
|---|---|---|
| **Agent developer** | A company building an agent product | A dependable computer-use backend and SDK |
| **Automation operator** | Responsible for internal process automation | Automating in-house software, execution logs, audit |
| **Desktop owner (human)** | End user of an agent product | The experience that "my agent and I share **one computer**." Connects directly to sign in, install, and operate |

### 3.2 Representative scenarios

**S1. Web search**
```text
"Open Chrome and search for Cocso"
→ launch_app("chrome") → screenshot → click(search box) → type("Cocso") → key("Enter") → screenshot
```

**S2. Sending a message from a desktop application**
```text
"Open the messenger and send a message to Hong Gil-dong"
→ launch_app("messenger")  # already signed in
→ screenshot → click(contact search) → type("Hong Gil-dong") → click(conversation)
→ click(input box) → type("Hello") → key("Enter")
```

**S3. Scheduled routine (triggered by an external agent scheduler)**
```text
Daily at 09:00
→ POST /v1/desktops/{id}/resume      # resume the existing Desktop
→ app.list to check actual state (the §7.4 contract) → reuse if alive, relaunch otherwise
→ search the news → summarize
→ reuse the messenger session → send the message
→ POST /v1/desktops/{id}/suspend     # back to sleep
```

**S4. Human intervention (handover)**
```text
The agent is stuck at a 2FA prompt
→ the agent notifies the user
→ the user opens the viewer → preempts the control lease → types → returns it to the agent
```

**S5. Human sets things up first, agent takes over**
```text
The user connects right after creating the Desktop
→ installs the work messenger and signs in (including 2FA)
→ installs the corporate VPN client, registers certificates
→ disconnects
→ from then on the agent uses that signed-in state
```
The agent can use signed-in applications **without ever being given the credentials.** **This is the core benefit of the co-ownership model.**

**S6. Human reviews and edits the result**
```text
The agent produces an Excel report and saves it
→ the user connects, opens the same file, and edits it directly
→ downloads the file, or gives the agent a follow-up instruction
```

---

## 4. Competitive Analysis

### 4.1 The alternatives

| Product | What it provides | Persistence | Desktop apps | Shared with a human |
|---|---|---|---|---|
| **E2B** | Sandboxes for code execution, mostly headless | Session only | ❌ | ❌ |
| **Browserbase** | Managed headless/headful browsers | Partial browser profile | ❌ Browser only | Live view for observation |
| **Anthropic Computer Use** | The model's **ability** to operate a screen, not infrastructure | — | Depends on the host environment | — |
| **Scrapybara / Kernel and similar** | Remote desktop or browser instances | Limited | Partial | Limited |
| **RPA (UiPath and similar)** | Rule-based desktop automation | On-premise PC/VM | ✅ | Occupies a person's PC |
| **Iapetus** | **Persistent desktop + co-ownership** | ✅ Weeks to months | ✅ | ✅ Equal authority |

### 4.2 Three axes of difference

1. **Persistence** — most alternatives discard the environment when the job ends. Iapetus **carries login sessions, installed applications, and files into the next run.** In practice, no longer signing in every time is the largest difference.

2. **Desktop applications** — browser automation products handle only the web. A substantial share of Korean office work — desktop messengers, Excel, internal ERP, government software — is not on the web. **Automating software that has no API** is the market Iapetus opens.

3. **Co-ownership** — elsewhere the human is an observer, or the human and the bot use separate environments. Iapetus has them **share one computer with equal authority.** A human signs in once and the agent uses that session (S5) — automation without handing over credentials.

### 4.3 Where we lose (an honest list)

| Area | Who wins | Why |
|---|---|---|
| **Raw code-execution speed and cost** | E2B, Vercel Sandbox | Without a desktop session and display server, the alternative is lighter and cheaper. If you only need to run code, there is no reason to use us |
| **Large-scale parallel web scraping** | Browserbase, headless clusters | Headless is far cheaper and far denser than a GUI |
| **Deterministic internal automation** | Established RPA | There are clearly domains where rule-based automation is more stable and easier to audit than an LLM |
| **Very low latency remote work desktops** | VDI (Citrix, AWS WorkSpaces) | VDI has been optimized for humans for decades. We are agent-first by design |
| **Lowest absolute cost** | Every headless alternative | The cost of keeping one desktop alive is structurally high |
## 5. Core Concepts and Data Model

### 5.1 Concept hierarchy

```text
Organization
  └── Project
        └── Desktop          # the persistent computer (first-class resource)
              ├── Owners     # human(s) + agent(s); all hold equal authority
              ├── Image      # which OS and applications the template carries
              ├── Volume     # persistent disk (home directory, application data)
              ├── OS Session # exactly one, shared by human and agent
              ├── Apps       # catalog shortcuts + running processes
              └── Session    # a control connection (human or agent)
```

### 5.2 Ownership and actor model

**A Desktop can carry several Owners, and their authority is identical regardless of kind.**

```json
{
  "desktop_id": "dsk_01H8XK",
  "owners": [
    { "type": "human", "id": "usr_kim",    "role": "OWNER" },
    { "type": "agent", "id": "agent_123",  "role": "OWNER" }
  ]
}
```

| Capability | Human Owner | Agent Owner |
|---|---|---|
| OS authority | root / Administrator | root / Administrator (identical) |
| Run and install programs | ✅ | ✅ |
| Full filesystem | ✅ | ✅ |
| System settings and reboot | ✅ | ✅ |
| How they connect | Browser viewer (WebRTC) | Computer API (REST/WS/MCP) |
| Delete the Desktop | ✅ | ✅ (requires `confirm_name`) |
| Add or remove Owners | ✅ | ❌ **humans only** |

**There are exactly two asymmetries.** ① Only a human may change the Owner list (below). ② Only a human may preempt the control lease (§5.6). Every other authority is identical. An agent cannot invite another agent or remove a human Owner. Authority is equal, but **the authority to grant authority stays with people.**

#### Desktop type — personal and shared are different products

When several people are Owners, **login sessions are shared, so B sees A's personal accounts.** That is not a bug but a consequence of the single-OS-session design, so the intended use is split into types to set expectations.

| `desktop_type` | Human Owners | Purpose | Personal account sign-in |
|---|---|---|---|
| **`personal`** (default) | **1** | Individual work automation | ✅ Allowed |
| **`shared`** | Many | Team bots, internal system automation | ⚠️ **Discouraged.** Use organizational accounts only |

- Adding a second human Owner to a `personal` Desktop requires converting it to `shared`, and the conversion requires **explicit acknowledgement that existing login sessions become visible to the new Owners.** It cannot be undone.
- A `shared` Desktop shows a persistent banner in the viewer: **"This computer is shared with N people. Do not sign in with personal accounts."**
- We do not technically block personal sign-in, because there is no way to tell which account is personal. **The platform's responsibility ends at the warning and the type distinction; beyond that it is organizational policy.**

**One shared OS session:** a Desktop contains exactly **one** OS user account and one desktop session. We do not create separate accounts for the human and the agent to log into simultaneously.

| Why not separate accounts | Consequence |
|---|---|
| Application login state (messenger, Chrome profile) belongs to a user profile | Separate accounts mean the agent cannot use the human's logins — which removes the product's reason to exist |
| GUI automation assumes exactly one active display | Separate sessions split the coordinate space and focus |
| Attribution is by **control session (actor)**, not OS account | Who did what is traced through the §9.4 audit log |

### 5.3 Desktop

A persistent resource belonging to its Owners. It does not disappear when nobody is connected.

```json
{
  "id": "dsk_01H8XK",
  "name": "sales-agent-desktop",
  "project_id": "prj_abc",
  "os": "linux",
  "image": "iapetus/linux-xfce-base:2026.08",
  "spec_tier": "standard",
  "display": { "width": 1920, "height": 1080, "dpi": 96 },
  "desktop_type": "personal",
  "privilege_mode": "owner",
  "os_user": { "name": "iapetus", "sudo": "nopasswd" },
  "owners": [
    { "type": "human", "id": "usr_kim",   "role": "OWNER" },
    { "type": "agent", "id": "agent_123", "role": "OWNER" }
  ],
  "status": "ACTIVE",
  "persistent": true,
  "idle_timeout_sec": 900,
  "auto_suspend": true,
  "labels": { "team": "sales" },
  "created_at": "2026-08-15T09:00:00Z",
  "last_active_at": "2026-08-15T09:42:11Z"
}
```

### 5.4 Desktop state machine

```text
              create
                │
                ▼
          PROVISIONING ──fail──► ERROR ──delete──┐
                │                                │
                ▼                                │
      ┌──────► ACTIVE ◄──┐                       │
      │      (booted,     │                      │
      │   OS + apps live) │ recovery              │
      │         │        │                       │
      │         └─► DEGRADED (limited, §19.4)     │
      │         │                                │
   resume       │ idle_timeout / explicit suspend │
      │         ▼                                │
      └──── SUSPENDED ──────────────┐            │
                │                   │            │
             restore            delete           │
                │                   ▼            ▼
                └──────────── DELETING ──24h──► TERMINATED
                              (grace period)
```

| State | Meaning | Billing |
|---|---|---|
| `PROVISIONING` | Being created | None |
| `ACTIVE` | Booted. OS and applications are alive whether or not anyone is connected | Compute |
| `DEGRADED` | A sub-state of ACTIVE: running but functionally limited (daemon protocol mismatch, Guest Token renewal failure). §19.4 | Compute |
| `SUSPENDED` | Asleep after a memory snapshot; disk retained | Storage only |
| `ERROR` | Unrecoverable | Storage only |
| `DELETING` | Deletion requested, within the 24-hour grace period (§10.3). API access blocked, volume still present | Storage only |
| `TERMINATED` | Deletion complete | None |

**Why READY and RUNNING are not separate states:** "is anyone connected" is already expressed by the Session resource. Splitting the Desktop state in two only complicates billing and orchestration and means nothing to an agent. Connection status is read from `sessions[]` and `last_active_at`.

**Idle determination:** `idle_timeout_sec` counts **from the moment the number of active Sessions reaches zero.** Any attached party, human or agent, resets the counter. If the agent must run background work (a build, a download) with nobody connected, disable it with `auto_suspend: false`.

**Core rule:** resuming from `SUSPENDED` to `ACTIVE` **restores the running applications and window layout as they were** (see §7.4 for the exact guarantee).

### 5.5 App

An App is **a convenience shortcut, not a restriction**. Programs absent from the catalog can be launched directly by path or command.

```json
{
  "key": "acme_messenger",
  "name": "Acme Messenger",
  "os": ["windows"],
  "launch": { "type": "exec", "command": "C:\\Program Files\\Acme\\Messenger.exe" },
  "singleton": true,
  "window_match": { "title_regex": "^Acme Messenger" },
  "installed": true,
  "state": "RUNNING",
  "pid": 4820,
  "windows": [{ "id": "win_1", "title": "Acme Messenger",
               "bounds": { "x": 100, "y": 100, "width": 420, "height": 720 } }]
}
```

### 5.6 Session

A control connection held by a human or an agent. **Many Sessions may exist at once, but the input control lease is always held by exactly one.**

```json
{
  "id": "ses_9f2",
  "desktop_id": "dsk_01H8XK",
  "actor": { "type": "agent", "id": "agent_123" },
  "control": "WRITE",
  "lease_expires_at": "2026-08-15T09:47:00Z",
  "heartbeat_interval_sec": 30,
  "started_at": "2026-08-15T09:42:00Z"
}
```

| `control` | Meaning |
|---|---|
| `WRITE` | Holds the input lease. May move the mouse, type, and launch applications |
| `READ` | Observation only. screenshot, streaming, and `app.list`; no input |

#### Control lease arbitration

Authority is equal, but **there is physically one keyboard.** If both parties type at once the screen is corrupted. So what is arbitrated is not authority but **input order.**

| Situation | Outcome |
|---|---|
| Nobody holds `WRITE` | The first requester acquires it immediately |
| Agent holds it, a human requests | **Immediate preemption.** The agent drops to `READ` and receives `control.revoked` |
| Human holds it, an agent requests | **No preemption.** Handover happens automatically after `human_idle_sec` (default **300s**) without input, or when the human explicitly releases. At 60s the agent would seize the keyboard mid-2FA in S4, while the person checks a text message or switches applications |
| **Agent holds it, another agent requests** | First come, first served. **Immediate rejection** (`CONTROL_HELD` + `retry_after_sec`). Only humans may preempt |
| Human holds it, another human requests | First come, first served. **Immediate rejection** (`CONTROL_HELD`) — the no-queue policy applies between people as well |
| Lease expires (heartbeat lost) | Reclaimed after three missed intervals. An in-flight action runs to completion first |

**Why the asymmetry:** not because a human outranks an agent. A human cannot wait in a queue, and intervention usually happens precisely when the agent is stuck or doing the wrong thing. The reverse direction — an agent pushing a human out — would discard that person's input, so it is forbidden.

**Preemption during an action:** an individual in-flight action is never interrupted, which is what prevents half-typed strings. The current action completes, the lease transfers, and subsequent calls return `CONTROL_LOST`.

**Handover resets input state.** Because `key.down` and `mouse.down` are separate actions (§7.2), an agent preempted after sending `key.down ctrl` would **leave Ctrl held, so everything the human types is interpreted as a shortcut.** Immediately before handover `iapetusd` force-releases every held key and mouse button (issuing the corresponding `key.up` events) and hands the new holder a clean input state.

#### Concurrent requests — rejection, not a queue

Two parties directing the same Desktop at once genuinely happens. The typical case is **a user making a live request while the 09:00 scheduled routine is running.**

**Decision: v1 rejects immediately rather than queuing.**

```jsonc
// POST /v1/desktops/{id}/sessions  → 409
{
  "error": "CONTROL_HELD",
  "holder": { "type": "agent", "id": "agent_123", "since": "2026-08-15T09:00:04Z" },
  "retry_after_sec": 30,
  "hint": "A human actor can preempt immediately via control/acquire"
}
```

| Option | Adopted | Reasoning |
|---|---|---|
| **Immediate rejection + `retry_after_sec`** | ✅ v1 | The caller learns the situation and decides for itself. Simple to implement. No unbounded waiting |
| Server-side queuing | ❌ | Agent work can run for minutes, so queue waits exceed request timeouts. Queue ordering, starvation, and cancellation are all new complexity |
| Forced preemption between agents | ❌ | In-flight work breaks with the screen in an intermediate state and cannot be recovered |

**Guidance for agent developers:** when you need concurrency, **split Desktops.** That is this platform's scaling model. A design in which several agents share one Desktop works against the fact that a GUI is a shared resource.

```text
❌  1 Desktop ← Agent A, Agent B, Agent C   (contention)
✅  3 Desktops ← Agent A, B, C separately   (parallelism)
```

**Humans are the sole exception.** A human may preempt (table above), because splitting Desktops between people is meaningless — what the user wants to see is **the very screen the agent is using right now.**

---
## 6. System Architecture

### 6.1 Overall structure

```text
┌──────────────────────────────────────────────────────┐
│                External Agent Runtime                │
│              (LLM / Planner / Memory)                │
└───────────────────────┬──────────────────────────────┘
                        │  SDK / REST / WS / MCP
                        ▼
┌──────────────────────────────────────────────────────┐
│                   Iapetus Control Plane              │
│  ├─ API Gateway (authn/z, rate limit, audit)         │
│  ├─ Desktop Orchestrator (lifecycle, scheduling)     │
│  ├─ Session Manager (lease arbitration, routing)     │
│  ├─ Owner / IAM (ownership, token issuance)          │
│  ├─ Image & App Catalog                              │
│  ├─ Policy / Secret Store                            │
│  └─ Audit & Recording Store                          │
└───────────────────────┬──────────────────────────────┘
                        │  internal control channel (mTLS)
                        ▼
┌──────────────────────────────────────────────────────┐
│                   Iapetus Data Plane                 │
│                                                      │
│   ┌───── Desktop Runtime (microVM / Container) ────┐ │
│   │  iapetusd (in-guest daemon, Rust)              │ │
│   │   ├─ Input Driver   (click/type/key/scroll)    │ │
│   │   ├─ Capture Driver (screenshot/stream)        │ │
│   │   ├─ App Launcher   (arbitrary exec + install) │ │
│   │   ├─ FS Bridge      (full path access)         │ │
│   │   └─ Shell Executor (root/sudo, on by default) │ │
│   ├────────────────────────────────────────────────┤ │
│   │  OS Layer:  Linux(XFCE) │ Windows              │ │
│   │  Apps:      Chrome, Terminal, Messenger, Excel │ │
│   │  Volume:    /home/iapetus | C:\Users\iapetus   │ │
│   └────────────────────────────────────────────────┘ │
│                       │                              │
│              ┌────────┴─────────┐                    │
│              │ Stream Gateway   │  encode once,      │
│              │ (SFU, WebRTC)    │  send N times      │
│              └────────┬─────────┘                    │
└───────────────────────┼──────────────────────────────┘
                        │ SRTP / DataChannel
                        ▼
                  human browser viewer
```

### 6.2 OS abstraction

An agent never needs to know whether it is on Linux or Windows. The same Computer API maps to a native implementation inside each OS's `iapetusd`.

| Computer API | Linux (XFCE/X11) | Windows |
|---|---|---|
| `screenshot` | X11 XGetImage / PipeWire | DXGI Desktop Duplication |
| `click`, `move` | XTEST | SendInput |
| `type`, `key` | XTEST + IME | SendInput + IME |
| `scroll` | XTEST button 4/5 | SendInput WHEEL |
| `launch_app` | exec + .desktop | CreateProcess |
| `window.*` | EWMH / wmctrl | Win32 Window API |
| `clipboard.*` | X11 selection | Win32 Clipboard |

**Differences are surfaced, not hidden:** anything that does not exist on a given OS is declared up front through `capabilities` and returns `UNSUPPORTED_ON_OS` when called. It is never silently ignored.

### 6.3 Screen pipeline — what the human sees and what the agent sees

**Requirement:** a human must be able to **watch the Desktop live and operate it directly**, while an agent needs **a still frame to reason about**. The two requirements have nothing in common, so the pipelines are separate — but **there is only one capture source.**

| | Human (viewer) | Agent |
|---|---|---|
| Wants | Continuous, unbroken motion | One exact frame at a point in time |
| Latency | Lower is better (< 200ms) | Tolerant (< 500ms) |
| Quality | Lossy is fine; motion matters | **Text must be legible** — an LLM reads the characters |
| Frequency | 30fps continuous | Once per action, sporadic |
| Format | H.264 video stream | Single JPEG/PNG frame |

Agents are not given a video stream: an LLM looks at one frame, not a sequence. Conversely humans are not given burst screenshots, which cost ten times the bandwidth.

#### Choosing the transport

| Candidate | Latency | Native in browser | Bandwidth | Verdict |
|---|---|---|---|---|
| **WebRTC** | 50–150ms | ✅ | Adaptive 0.5–8Mbps | ✅ **Adopted** |
| VNC / RFB (noVNC) | 200–500ms | ❌ (needs a JS decoder) | Inefficient (raw/tight) | ❌ Worse latency and quality |
| RDP | 100–200ms | ❌ (gateway required) | Good | ❌ Windows-biased, needs a browser translation layer |
| WebSocket + JPEG diff | 150–400ms | ✅ | Poor (keyframe-heavy) | △ **Fallback only** |
| HLS / DASH | 2–10s | ✅ | Good | ❌ Not interactive |

**Why WebRTC**
1. The browser already contains the decoder, jitter buffer, and congestion control. There is nothing to build.
2. It **lowers the bitrate automatically** when the network degrades. A JPEG scheme requires building that yourself, usually badly.
3. Input can ride the same connection over a DataChannel, so **screen and input share one RTT.**
4. Recording (§7.5 V-08) can be muxed from the same stream **without re-encoding**. Masking happens before encoding, so it does not break this property (§10.2).

**Fallback:** for corporate networks that block UDP, ① TURN over TCP/443, then ② WebSocket JPEG diff (5–10fps — usable but not smooth). On entering fallback the viewer shows a badge so the user understands why it feels slow.

#### The pipeline

```text
        ┌──────────────── Guest (iapetusd) ────────────────┐
        │                                                  │
        │   X11 XDamage / DXGI Duplication                 │
        │        (only changed regions are signalled)      │
        │                    │                             │
        │                    ▼                             │
        │            Frame Source  ◄── single capture source│
        │           (RGBA ring buffer)                     │
        │              │            │                      │
        │   ┌──────────┘            └──────────┐           │
        │   ▼                                  ▼           │
        │ Still Encoder                  Video Encoder     │
        │ (JPEG/PNG, on demand)      (H.264, only if watched)│
        │   │                                  │           │
        └───┼──────────────────────────────────┼───────────┘
            │ HTTPS (via Control Plane)         │ SRTP → Stream Gateway (SFU)
            ▼                                  ▼
      Agent (screenshot)                 human browser (live)
                                              ▲
                                    DataChannel │ input (mouse/keyboard)
```

**Three design decisions**

1. **One capture source, two encoders.** Capture is expensive (1080p RGBA is an 8MB copy per frame) and doing it twice is waste. Both encoders read one ring buffer.

   **But a cached frame must not simply be returned.** Capture is driven by XDamage events and lags 5–15ms, so an agent that calls `screenshot` immediately after `click` **can receive the pre-click screen.** That is a correctness fault: the agent then reasons from a world state that never existed.

   **Freshness contract:** `screenshot` returns only **a frame captured after the preceding action completed.** If the newest frame in the ring buffer is older than that, a fresh capture is forced and awaited. The response's `taken_at` proves it.

   **State changes with no input cannot be covered by this contract.** A page finishing its load, a dialog raised by `shell.exec`, a window rendering after `app.launch` — none has an action to anchor against. There the guarantee extends only to "after the call began," and **the agent must wait explicitly with `wait_for(screen_stable)`.** The platform does not guess: only the agent knows what it is waiting for.

2. **No viewers, no video encoder.** Of 10,000 Desktops, typically a few dozen are being watched. With no viewer the H.264 encoder is shut down entirely and capture runs purely off XDamage events, converging to zero CPU when nothing changes. **Without this, streaming costs more than compute.**

3. **Encode once, send N times.** However many observers there are, encoding happens once. `iapetusd` sends the encoded RTP packets to the stream gateway (`iapetus-stream`) exactly once, and the gateway replicates to each viewer as an SFU. Guest CPU is independent of observer count.

**The guest is not a WebRTC peer of the browser.** The guest transmits only to the gateway, and the browser peers only with the gateway. Direct P2P is not adopted: ① it exposes guest IPs, and ② it would require separate encoding per observer, which breaks decision 3 above. The STUN/TURN ladder under "NAT traversal" below applies **only to the browser ↔ gateway segment** — guest to gateway is an intra-host path where ICE is unnecessary.

#### Multiple observers versus bitrate adaptation

Applying GCC congestion control to a single encoded stream lets **the slowest viewer drag down everyone's quality.** Lowering resolution instead (simulcast or spatial SVC) requires encoding once per layer, which destroys the §12.4 capacity model.

**Decision: temporal layering only.**

**This decision forces the encoder.** x264 has no temporal-layer API and therefore cannot be used; the choice is **OpenH264** (`iTemporalLayerNum`) or NVENC. OpenH264 is slower than x264 `veryfast`, so **encoding cost rises to 1.2–1.8 vCPU.** The layer structure itself is nearly free, but the cost of changing encoders is real, and §12.4's CPU reservation and concurrent-observation cap are derived from this figure.

```text
Guest encodes a hierarchical-P structure once (layers themselves cost ~0)
   L0: 7.5fps  (base layer)
   L1: +7.5fps → 15fps
   L2: +15fps  → 30fps

The gateway drops upper layers per viewer:
   fast viewer  → L0+L1+L2 = 30fps
   slow viewer  → L0 only  = 7.5fps  (resolution and sharpness preserved)
```

Layer IDs travel in a proprietary header on the guest → gateway segment, which is free to deviate since it is not standard WebRTC. The gateway drops packets by header alone and **never decodes.**

| | Temporal layers (adopted) | Simulcast / spatial SVC (rejected) | Gateway transcoding (rejected) |
|---|---|---|---|
| Encoding cost | 1× (though the baseline itself is 1.5× after moving to OpenH264) | 2–3× | × number of viewers |
| What a slow viewer gets | Lower frame rate, **original resolution** | Lower resolution | Arbitrary |
| Text legibility | ✅ Preserved | ❌ Breaks | △ |

**Not sacrificing resolution is the point.** As established at the top of this section, a human has to read the characters on screen, and since a desktop is mostly static, a lower frame rate is barely noticeable in practice.

#### Codec and bitrate

| Item | Default | Rationale |
|---|---|---|
| Codec | H.264 **High Profile** (4:2:0) | Baseline lacks CABAC and the 8×8 transform, which hurts screen content especially. High renders text visibly sharper at the same bitrate and every target browser decodes it |
| Resolution | 1920×1080, identical to the Desktop | Scaling smears text past the point a human can read it |
| Frame rate | 30fps ceiling, **0fps when nothing changes** | A desktop screen is static most of the time |
| Bitrate | 1.5Mbps default, adaptive 0.5–8Mbps | GCC congestion control adjusts automatically |
| Keyframes | Every 2s, plus immediately when a viewer joins | Removes the wait for newcomers |
| Encoder | **OpenH264** (`iTemporalLayerNum=3`), latency-first settings | **x264 cannot emit temporal layers** — it has no SVC or reference-list control API. The multi-observer decision below forces this choice |
| GPU tier encoder | NVENC (layer support) | The same layer structure in hardware |

**Text sharpness — acknowledging the 4:2:0 limit and working around it.**

H.264 4:2:0 discards three quarters of the chroma resolution, so **the edges of colored text — syntax-highlighted code, links, warning text — smear.** This is a separate loss that lowering QP does not fix. The 4:4:4 profiles are not adopted because browser decode support is uneven.

**The response: treat motion and stillness differently.**

| State | Handling |
|---|---|
| Screen in motion | H.264 High 4:2:0. OpenH264 `iUsageType=SCREEN_CONTENT_REAL_TIME`, `iRCMode=RC_BITRATE_MODE`, `iMaxQp=32`, `uiIntraPeriod=60`. Smoothness first |
| **Still for ≥ 500ms** | Send **only the changed region** once more as high-quality WebP (q=95) and overlay it. Budget below |

**Overlay budget — not hand-waved as "negligible."**

A full 1080p frame as lossless PNG is 1–3MB, which at the default 1.5Mbps takes 5–16 seconds. That is unusable as-is.

| Rule | Value |
|---|---|
| Scope | **Bounding box of the region changed since the last keyframe**, not the full screen |
| Format | WebP q=95 (not lossless; sufficient to preserve text edges) |
| Size cap | **200KB.** Above that, skip and wait for the next still event |
| Transport | **DataChannel**, not the video track (a separate SCTP stream from input). The browser composites it on a canvas layer above the `<video>` element |
| Minimum interval | 2s, so repeated stop/start does not produce a burst |

**It contends with input for bandwidth.** The overlay shares the DataChannel with input. A separate SCTP stream avoids head-of-line blocking but **the congestion window is shared**, so input round trips (20–50ms) can rise while an overlay is in flight. That is why the overlay is sent **only after the screen has been still for 500ms** — a period in which the person has stopped operating, so the added input latency is not felt. It is never sent during active operation.

200KB is roughly 1 second at 1.5Mbps and 3 seconds even at the 0.5Mbps floor. **A slow viewer may receive the overlay late or not at all if the cap is hit.** So convergence to identical sharpness across viewers is not guaranteed: fast viewers get it almost immediately, slow ones get it late or stay on the H.264 frame.

**How the three optimizations interlock** — a different one applies in each screen state.

| Screen state | Frame rate | Temporal layers | Still overlay (WebP q=95) |
|---|---|---|---|
| In motion | Up to 30fps | Active (slow viewers at L0, 7.5fps) | Not sent |
| Just stopped (< 500ms) | 0fps (no new frames) | Moot | Not sent |
| **Still for ≥ 500ms** | 0fps | Moot | **Sent once.** Whether it arrives depends on the 200KB budget and the viewer's link |

Once the screen is static, temporal layering loses its meaning. Viewers with enough bandwidth receive the overlay and see a sharp screen; viewers limited by the 200KB budget or their link stay on the H.264 frame. **Convergence is not complete, but frame-rate differentiation exists only during motion, and while static the gap narrows as far as bandwidth allows.**

The moment a user reads text is always a moment the screen is still. Paying for quality only then satisfies the legibility requirement and the encoding budget at once.

> Parameters are given in OpenH264 terms. x264's `tune=zerolatency` and `qpmax` do not exist in this encoder — changing encoders changes the setting names too.

#### Latency budget (same region, wired)

| Segment | Target | Note |
|---|---|---|
| Screen change → capture complete | 5–15ms | XDamage-driven |
| Capture → H.264 encode (OpenH264 SW) | **12–30ms** | ~1.5× x264. Both throughput and per-frame latency rise |
| Guest → stream gateway | 2–5ms | Same host/rack |
| Gateway → browser (RTT/2) | 10–40ms | Intra-region |
| Browser jitter buffer + decode | 20–60ms | Browser-controlled |
| Frame-rate quantization (L2, 30fps) | 0–33ms | Viewers receiving all layers |
| Display refresh (vsync) | 8–16ms | Browser and monitor |
| **Total (glass-to-glass, 30fps viewer)** | **57–199ms** | **Effectively touching** the < 200ms KPI. See below |
| Frame-rate quantization (L0, 7.5fps) | 0–133ms | Layer-downgraded slow viewers |
| **Total (viewer downgraded to L0)** | **57–299ms** | The lower bound is identical — it is the case where a frame happens to be ready. **The upper bound greatly exceeds the KPI.** See below |

**Software encoding leaves no KPI headroom.** An upper bound of 199ms clears a 200ms target by one millisecond, which does not mean the target is met so much as that **one more adverse condition breaks it.** This is where the price of abandoning x264 to obtain temporal layers becomes visible.

**So the GPU tier is not only about observation density.** NVENC drops per-frame encoding latency to 5–10ms and the ceiling to ~179ms, which is where real headroom appears. Independently of §12.4's rule that observation ratios above 20% require GPU, **latency-sensitive use — remote operation rather than watching — should use the GPU tier even with few observers.**

**Downgraded viewers are outside the KPI.** A viewer dropped to L0 (7.5fps) spends 133ms on frame interval alone and cannot fit a 200ms budget. This is the designed trade: for a viewer on a poor network we **give up latency rather than resolution, so the text stays readable.** §2.2's streaming latency KPI therefore applies **only to viewers receiving all layers**, and the downgrade rate (target < 5%) is tracked as a separate metric.

The reverse path (input) is **20–50ms** over the DataChannel. What a person actually perceives, however, is the round trip of **input → applied in guest → capture → encode → display**, which is **75–240ms**. Feeling under 100ms requires regional proximity, and we accept that remote connections will feel the latency.

#### CPU cost and capacity

Target figures on a general instance without a GPU, to be confirmed by measurement:

| State | vCPU |
|---|---|
| No viewers, screen static | ~0.01 |
| No viewers, agent taking one screenshot per second | ~0.05 |
| One or more viewers, 1080p30 OpenH264 software encoding, 3 temporal layers | **1.2–1.8** |

**The encoder runs inside the guest, but its cost is not charged to the tenant's quota.** Doing so would make Chrome slow down at the exact moment a human starts watching — a self-inflicted wound. When observation begins **the host allocates an additional 1.8 vCPU to that Desktop for encoding**, separate from the tenant's 2 vCPU, and reclaims it when observation ends. Because that extra comes from a host reservation, it puts a ceiling on concurrent observation.

**So host capacity is determined by memory, and concurrent observation by reserved CPU.** The concrete figures live in §12.4 as the single source.

On the GPU tier (NVENC/QSV) the encoding CPU cost falls to roughly **0.05 vCPU per Desktop**. The bottleneck merely moves from vCPU to **the GPU's concurrent encoding session limit** (tens of sessions per GPU at 1080p30); if that limit is below **the `gpu` tier's 14 Desktops per host** (§12.4), it becomes the new ceiling. To be confirmed per GPU model. Observation-heavy workloads — training, audit, demos — should use the GPU tier.

#### Thumbnail previews

The dashboard has to show dozens of Desktops at once. Attaching WebRTC to all of them exceeds capacity immediately.

| Purpose | Method | Cost |
|---|---|---|
| List thumbnail | **320×180 JPEG every 10s, cached in object storage** | Guest CPU is negligible. **Storage and PUT cost are not** — 10,000 Desktops is 60,000 PUTs per minute |
| Card hover preview | 1fps JPEG polling | Low |
| Detail view | Full WebRTC stream | High |

**Thumbnails are generated only while someone is actually looking at a list.** Continuously rendering thumbnails for every Desktop when nobody has the dashboard open is waste. Thumbnails are screenshots, so they follow the §10.2 retention policy (24 hours by default).

The still encoder produces them at low resolution, uploads to S3, and a CDN serves them. **The video encoder is never woken.** A SUSPENDED Desktop shows the last thumbnail taken before it slept — the purpose is to let a user recognize which computer had what open, which does not need to be live.

#### Audio

| Version | Scope |
|---|---|
| v1 | **Unsupported.** A dummy sound device is present so applications do not fail on its absence |
| v2 | One-way Opus (Desktop → human), for notification sounds and checking video |
| v3 | Two-way (microphone → Desktop), for meeting-automation scenarios |

Audio is excluded from v1 because lip-sync alignment pressures the latency budget and the initial scenarios — search, messaging, business applications — do not need sound.

#### NAT traversal

```text
1st  ICE host/srflx (STUN)     → direct UDP.  lowest latency
2nd  TURN over UDP:3478        → relay. +10–20ms
3rd  TURN over TCP/TLS:443     → for corporate firewalls. +30–60ms
4th  WebSocket JPEG diff       → where WebRTC is blocked entirely
```

TURN credentials are **short-lived, HMAC-based, and issued alongside the Viewer Token** (15 minutes). There is no fixed TURN account, because a leaked one enables unbounded relay abuse.

### 6.4 Runtime selection

| Backend | Purpose | Kernel isolation | Memory snapshot | File sharing |
|---|---|---|---|---|
| Docker container (XFCE + Xvfb) | Local development, self-host, single organization | ❌ shared kernel | ❌ | bind mount |
| Kata Containers | (evaluated, not adopted) | ✅ | ❌ **none** | virtio-fs (qemu) |
| **Firecracker microVM** | **Multi-tenant SaaS (adopted)** | ✅ | ✅ | ❌ block devices only |
| Windows VM | Desktop applications (messenger, Excel, ERP) | ✅ | ✅ | — |

**Why not Kata:** Kata is an attractive path — kernel isolation while keeping OCI images as they are — but **it has no API to snapshot a running VM.** VM templating accelerates boot; it is not a checkpoint. Since process-preserving suspend/resume (§7.4) is a core claim of the product, we **orchestrate Firecracker directly** because it supports snapshots.

**Firecracker's cost:** it has no virtio-fs device, so file-level host-to-guest sharing is impossible. Both **the persistent volume and the `iapetusd` injection (§19.4) are therefore attached as block devices** — the daemon as a read-only squashfs image, the home directory as an ext4 volume. The file-level mounts a Kata/qemu path would have allowed are given up.

Images stay **a single OCI artifact**; the Firecracker path converts to a rootfs at boot. One Dockerfile is maintained.

#### Evaluated and rejected — multi-session (one OS, N desktop screens)

We evaluated running several X displays on one OS and giving each tenant only a screen (Windows RDS, Linux multi-seat). Sharing the kernel, binaries, and page cache drops **effective memory per session from 4GB to roughly 1.1GB.** On density alone it is attractive.

**Not adopted between different customers.** We grant guest root to both agents and humans (§7.3). Root bypasses Unix permissions by design, so dividing by directory or OS account is **organization, not isolation.**

| Isolation level | What it stops | For us |
|---|---|---|
| Directories + Unix permissions (DAC) | A non-root mistake | Meaningless once root is granted |
| **MAC (SELinux MCS / AppArmor)** | **Some deliberate root action** | **§7.3 grants the authority to dismantle it — see below** |
| user namespace / container | Deliberate non-root action | Shared kernel → root escapes via a kernel vulnerability |
| **microVM** | **Deliberate root action** | ✅ Adopted |

**MAC is not a straw man.** SELinux MCS exists precisely for this kind of in-OS multi-tenant separation — OpenShift uses it — and **the very fact that root bypasses DAC is why MAC was built.** So "root makes directories meaningless" is not an argument that disposes of MAC.

**The disposal comes from §7.3.** We promised kernel modules, drivers, `system.service`, and `sudo`/UAC elevation. That is equivalent to granting `CAP_MAC_ADMIN` and `CAP_SYS_ADMIN`, and a party holding those can lift the MAC policy applied to it. **Withhold them and §7.3's promise that "there is no application you cannot open" no longer holds.** OWNER mode and MAC are incompatible, and that is the real reason we climb all the way to microVM.

It is the same reasoning by which §9.1 forbids even containers for multi-tenancy. A weaker separation than a container cannot qualify.

**The density advantage also reverses for our workload.** Multi-session has no way to snapshot a single session's memory, so **it cannot suspend.** A session either stays resident or is killed and loses its state. The VM model, by contrast, drops idle Desktops to `SUSPENDED` and their RAM usage to zero.

RAM required for 10,000 owned Desktops:

| Activity ratio | VM-per-Desktop | Multi-session | |
|---|---|---|---|
| 1% | **0.4TB** | 10.7TB | VM 27× better |
| 5% | **2.0TB** | 10.7TB | VM 5.5× better |
| **28%** | 10.9TB | 10.7TB | Break-even |
| 100% | 39.1TB | **10.7TB** | Multi-session 3.6× better |

**This comparison is biased in multi-session's favour.** The VM side is charged its **4GB reservation** because of the no-overcommit rule (§12.4), while the multi-session side is charged **1.1GB of average actual usage** — granting the benefit of statistical multiplexing to one side only. Holding a session's peak safely on a swap-free host (300–400MB of desktop environment plus 800MB–1.5GB of Chrome) needs 1.8–2.2GB per session, so **a fair comparison puts break-even at 45–55%.** The figures below are the conservative reading, and the conclusion only strengthens.

**Break-even is at a 28% activity ratio, even read conservatively.** The S3 scheduled routine (a few minutes a day) runs at a single-digit activity ratio, where the VM model is 5–27× better. Multi-session wins only for "a Desktop that is almost always on," which is VDI territory and a market §4.3 concedes we lose.

**Note also that the table compares RAM only.** The VM model leaves a memory snapshot on local NVMe for every `SUSPENDED` Desktop — roughly 40TB at ten thousand, with host affinity attached (§7.4). At 1% activity it **trades 0.4TB of RAM for 40TB of disk**, which remains strongly favourable even accounting for the 5–10× price difference per GB. Giving up snapshots and preserving only the volume removes that cost but loses process preservation.

**Conclusion: keeping `auto_suspend` on by default (§13) is the source of this efficiency.** It trades more memory per Desktop for a zero idle cost, and for our workload that trade pays heavily.

#### Deferred — Desktop Group (v2)

The analysis above concerns **different trust domains.** Between Desktops belonging to **the same customer there is no reason to isolate**, and the conclusion changes.

In particular §5.6 advises splitting Desktops when concurrency is needed — and **Desktops split for that reason are by definition active together.** They wake and sleep together, so the benefit of per-Desktop suspend disappears and only the benefit of sharing remains.

| Three parallel agents | Memory |
|---|---|
| Three Desktops, each a VM | 12GB |
| **One OS with three displays** | **5.3GB** (2GB OS + 3 × 1.1GB) |

**2.3×.** A real gain.

**Design direction — the API model does not change.**

```text
A Desktop Group is a placement directive, not a new unit of operation.

  DesktopGroup grp_01H8
    ├── Desktop dsk_A  → display :1 in the guest OS
    ├── Desktop dsk_B  → :2
    └── Desktop dsk_C  → :3
```

- **Desktop remains the unit of the API.** Coordinates, control lease, audit, and viewer all correspond to exactly one Desktop, so the **shape** of §7.2 coordinates, §5.6 leases, and §9.4 audit is unchanged. Adding a display axis to the API would make all three two-dimensional and the cost would climb sharply.
- Only the runtime changes: `iapetusd` supervises N X displays inside the guest and maps each to a Desktop ID.
- Desktops in a group must share **the same project and the same Owner set.** A common trust domain is the precondition.
- **What changes is not the shape but the semantics.** Inside a group there is one OS, so **system-scope actions reach the whole group.** That is distinct from "there is no isolation" — it is visible in the API.

| Action | Meaning inside a group |
|---|---|
| `system.reboot` | Called on `dsk_A`, it **reboots B and C as well** |
| `system.service` / `system.env` | OS-global, so it applies to the entire group |
| `process.list` / `process.kill` | **Other Desktops' processes are visible and killable** |
| `app.list` | Installed and running applications of every Desktop in the group are mixed together |
| `spec_tier` | A per-Desktop field, but with one OS it **must be identical across the group** |
| `privilege_mode: restricted` | Enforced inside the guest, so it **cannot differ per display** |

When Group is actually built in v2, **this list is where the cost lives.** "It is only a placement directive" does not extend to system-scope actions.

**What is given up — and why this is v2.**

| Item | Consequence |
|---|---|
| Suspend granularity | **The whole group.** If one is active, all of them stay resident |
| Isolation | **None within the group.** Files, clipboard, and processes are all shared |
| Blast radius | One Desktop breaking the kernel takes the whole group down |
| `iapetusd` complexity | Multi-display supervision, per-display capture, encoder, and input |
| §7.2 single-monitor rule | The group itself does not break it — each Desktop still has one screen |

**It is excluded from v1 for reasons of validation order.** Complicating the runtime before demand for parallel execution is observed would mean disturbing the core path for a usage pattern we do not yet know exists. Like H-3 (human intervention uptake), **"group demand" is observed in Phase 2 and decided then.**

---
## 7. Functional Specification

### 7.1 Desktop lifecycle

| ID | Capability | Description | Priority |
|---|---|---|---|
| D-01 | Create Desktop | Create with image, tier, resolution, and **initial Owners**. Owners may be set with `project:manage` at creation only (§8.1) | P0 |
| D-02 | Get / list Desktops | Status, labels, running applications | P0 |
| D-03 | Suspend / resume | Sleep and wake via memory snapshot | P0 |
| D-04 | Restart | Reboot the guest OS, volume retained | P1 |
| D-05 | Delete | Full deletion including the volume, with confirmation | P0 |
| D-06 | Auto-suspend | Sleep automatically past `idle_timeout` | P0 |
| D-07 | Change resolution | Alter the display at runtime | P1 |
| D-08 | Snapshot / restore | Save and roll back to an arbitrary point | P2 |
| D-09 | Custom image | Register an image carrying organization-specific applications | P1 |

**Important policy:** deletion is irreversible and destroys the volume. The API requires the Desktop's name to be retyped via `confirm_name`.

### 7.2 Computer API (core)

The unified control interface an agent uses.

**Coordinate convention (mandatory):** every coordinate is an **absolute guest physical pixel**, origin at the top left `(0,0)`.
- `screenshot(scale=0.5)` reduces **only the transmitted image size.** The coordinate space does not change.
- Even when the viewer displays the screen scaled down, the client converts back to original coordinates before sending.
- Every response returns `display{width,height,dpi}`, the frame of reference for interpreting coordinates.
- **v1 is fixed to a single monitor.** Multiple monitors are handled in v2 by adding `screen_id` to coordinates; in v1 `screen.info.count` is always 1.

#### Observation

| Action | Parameters | Returns |
|---|---|---|
| `screenshot` | `format`, `quality`, `region?`, `scale?` | Image (base64/URL), `taken_at`, `display` |
| `cursor_position` | — | `{x, y}` |
| `window.list` | — | Window list (id, title, bounds, focused) |
| `screen.info` | — | Resolution, DPI, monitor count |

#### Input

| Action | Parameters |
|---|---|
| `mouse.move` | `x`, `y`, `duration_ms?` |
| `mouse.click` | `x?`, `y?`, `button` (left/right/middle), `count` (1/2) |
| `mouse.down` / `mouse.up` | `button` |
| `mouse.drag` | `from{x,y}`, `to{x,y}`, `duration_ms?` |
| `scroll` | `x?`, `y?`, `dx`, `dy` |
| `type` | `text`, `delay_ms?` (IME and Hangul support required) |
| `key` | `keys` (e.g. `"ctrl+c"`, `"Enter"`), `count?` |
| `key.down` / `key.up` | `key` |
| `secret.type` | `secret_ref` — types a stored secret. **The plaintext never enters the agent's context, the audit log, or a capture.** The limits of that protection are in §9.3 |

#### Applications and windows

| Action | Parameters | Description |
|---|---|---|
| `app.list` | `installed_only?` | Catalog + installed applications + running state |
| `app.launch` | `key` or `command`, `args?`, `cwd?`, `elevated?`, `wait_for_window?` | A catalog key **or an arbitrary executable path** |
| `app.focus` | `key` or `window_id` | Move focus |
| `app.close` | `key` / `pid`, `force?` | Terminate |
| `app.install` | `manager` (apt/winget/brew/msi), `package` or `url` | Install a program |
| `app.uninstall` | `package` | Remove |
| `process.list` / `process.kill` | `pid`, `signal?` | Direct process control |
| `window.move` / `window.resize` | `window_id`, bounds | |
| `window.maximize` / `minimize` | `window_id` | |

#### Files and clipboard

The filesystem is **fully accessible by path**, including system directories.

| Action | Parameters |
|---|---|
| `fs.upload` | `path`, `content` or a presigned URL |
| `fs.download` | `path` → presigned URL |
| `fs.list` | `path` |
| `fs.read` / `fs.write` | `path`, `content`, `mode?` |
| `fs.delete` / `fs.move` | `path`, `dest?` |
| `clipboard.read` / `clipboard.write` | `text` |

#### Shell and system

An agent **may use an administrator shell by default.**

| Action | Parameters | Description |
|---|---|---|
| `shell.exec` | `command`, `cwd?`, `env?`, `elevated?`, `timeout_ms` | Synchronous; returns stdout, stderr, exit code |
| `shell.spawn` | `command`, `detach?` | Long-running process; returns `pid` |
| `shell.stream` | `pid` | Stream output while running |
| `system.env` | `set` / `get` | Environment variables |
| `system.service` | `name`, `action` (start/stop/enable) | Service control |
| `system.reboot` | — | Reboot the guest, volume retained |

#### Composite actions

| Action | Description |
|---|---|
| `act` | Run an array of actions in sequence and return a final screenshot, reducing round trips |
| `wait_for` | Wait for a screen change, a window, or a fixed duration |

**`act` is not atomic.** GUI work cannot be rolled back — if the third action fails, the first two are already on screen. The semantics are therefore:

- **Fail fast**: stop at the failing action. The remainder do not run.
- **Return partial results**: the results up to that point plus a `failed_at` index.
- **Always return a screenshot of the failure**: the agent's only basis for judging how far the screen got.
- Retry is the agent's responsibility. The platform never retries automatically.

```json
{
  "results": [
    { "type": "app.launch", "ok": true },
    { "type": "mouse.click", "ok": false, "error": "ACTION_TIMEOUT" }
  ],
  "failed_at": 1,
  "screenshot": { "url": "..." }
}
```

**`act` example**
```json
{
  "actions": [
    { "type": "app.launch", "key": "chrome", "wait_for_window": true },
    { "type": "mouse.click", "x": 640, "y": 120 },
    { "type": "type", "text": "Cocso" },
    { "type": "key", "keys": "Enter" },
    { "type": "wait_for", "mode": "screen_stable", "timeout_ms": 5000 }
  ],
  "return_screenshot": true
}
```

### 7.3 Authority model — humans and agents are both Owners

**Principle: nobody is constrained inside the Desktop.**

Human Owners and agent Owners both hold `OWNER` authority. Every row below applies to **both**:

| Authority | Guarantee |
|---|---|
| Run arbitrary programs | `app.launch(command=...)` and `shell.exec`, with no path restriction |
| Install and remove programs | `app.install` (apt / winget / msi / direct download) |
| Full filesystem | Read, write, and delete including system directories |
| Administrator rights | `sudo` / UAC elevation. Configured NOPASSWD, so there is no password prompt at all |
| System settings | Network, time zone, registry, services, drivers |
| Process control | Inspect and kill any process |
| Browser profile | Install extensions, add certificates |
| Reboot the guest | `system.reboot` |

**Why:** per-application allowlists always break in real automation. An internal ERP executable moves after an update, an Excel macro invokes a separate runtime, an installer creates a temporary executable. Requiring each of these to be registered makes the product unusable. Instead we **treat the Desktop itself as a disposable trust boundary** and control it from outside rather than inside (§9.1).

**The App catalog's role changes:** it is now **a discovery tool, not a blocking mechanism.** It exists so an agent can call `app.list` and learn what is installed on this computer and how to open it. Absence from the catalog does not prevent execution.

#### Principles for third-party applications

Desktop application automation carries terms-of-service risk. Three principles minimize the exposure.

| Principle | Content | Reasoning |
|---|---|---|
| **1. We do not distribute applications** | Third-party applications are not preinstalled in base images. The user installs them (S5), or the customer organization bakes them into its own image | Removes any exposure to distribution and redistribution clauses. The installing party is always the customer |
| **2. The user's own account only** | Automation targets **the account of that Desktop's own owner.** Operating other people's accounts, running accounts in bulk, and using the service to sell or rent accounts are prohibited by the terms | What most service terms object to is not automation itself but **account misuse and bulk activity** |
| **3. No dependence on any particular application** | An application is one entry in a catalog; the platform keeps working if any of them is blocked | Mitigates R-01. The product narrative is not tied to any application's name |

**Not naming specific commercial applications in this document follows from the same reasoning.** The capability is "desktop application automation," not "automating a particular application."

**Default catalog (preinstalled)**

| key | Linux | Windows |
|---|---|---|
| `chrome` | ✅ | ✅ |
| `terminal` | ✅ | ✅ (PowerShell) |
| `files` | ✅ | ✅ (Explorer) |
| `text_editor` | ✅ | ✅ |
| `vscode` | ✅ | ✅ |
| *(corporate messenger)* | Organization image | Organization image |
| `excel` | ❌ | ✅ (license required) |
| `custom_erp` | Organization image | Organization image |

**Optional restriction (off by default):** for regulated customers, project policy can enable `restricted` mode. Only in that mode is an allowlist enforced and do `APP_NOT_ALLOWED` / `POLICY_BLOCKED` occur. The default is `owner` mode, which blocks nothing.

```jsonc
// Default — full authority for both agents and humans
{ "privilege_mode": "owner" }

// Opt-in — for regulated industries
{
  "privilege_mode": "restricted",
  "allowed_apps": ["chrome"],
  "shell_exec": false
}
```

### 7.4 Persistence

| What | How it survives |
|---|---|
| Home directory / user profile | Persistent volume |
| Browser cookies, sessions, extensions | Persistent volume (Chrome profile) |
| Application login state (messengers and similar) | Persistent volume + suspend snapshot |
| Running processes and window layout | Suspend memory snapshot |
| Downloaded files | Persistent volume |

**The guarantee depends on the runtime.**

| Runtime | `restart` | `suspend` / `resume` |
|---|---|---|
| **Firecracker / Cloud Hypervisor** microVM (SaaS) | Volume preserved | Volume + processes + window layout preserved (**see constraints below**) |
| Kata Containers | Volume preserved | **Volume only.** Kata has no API to snapshot a running VM — templating accelerates boot, it is not a checkpoint |
| Docker (self-host) | Volume preserved | **Volume only.** The CRIU dependency is unstable, so v1 does not support it |
| Windows VM | Volume preserved | Volume + processes preserved |

**Isolation and snapshotting are different features, and the runtime choice is their intersection.** Kata is strong on isolation but has no snapshot. So the SaaS path, which needs process preservation, **orchestrates Firecracker (or Cloud Hypervisor) directly** (§19.2).

**Real constraints on restoring a memory snapshot — all of them must be designed for**

| Constraint | Consequence | Response |
|---|---|---|
| **CPU feature compatibility** | A snapshot restores only on a host with the same CPU template | Separate host pools per CPU template; pin a template label to the Desktop |
| **Host affinity** | A snapshot on local NVMe resumes only on that host (§12.2 RB-4) | If that host has no free memory the resume fails → return `NO_RESUME_CAPACITY`, then **discard the snapshot and cold boot elsewhere** |
| **Guest clock frozen** | On restore the clock is days stale, breaking TLS sessions, JWT expiry, and cookie validation | **`iapetusd` forces a clock resync immediately after restore** and holds actions until it completes. Without this step §2.2's login-preservation KPI does not hold |
| **Dead TCP sockets** | The guest wakes with every connection severed | Messengers and browsers reconnect. "The window layout is intact" is true; "the connections are intact" is not |

**SUSPENDED is not free.** Ten thousand 4GB Desktops is roughly 40TB of snapshot storage, with host affinity attached. §12.4's "SUSPENDED unlimited" means **as long as the storage is paid for**, not that placement is unconstrained.

On self-hosted Docker, `suspend` is effectively `stop` and applications restart on resume. **Login state lives on the volume and survives either way** — that is the product's core value, and process preservation is an additional optimization.

**Agent contract:** after resuming, always confirm actual state with `app.list`. Never assume that "Chrome was up before the suspend, so it is up now."

### 7.5 Human access — the full-access viewer

A human is **an equal Owner, not an observer.** The viewer must be **a remote desktop**, not an "agent monitoring screen." The technical pipeline was settled in §6.3; this section specifies what the user sees.

**Premise:** a user must be able to **see their Desktop at any time.** If they cannot watch what the agent is doing, they can neither trust nor correct it. Visibility is not an extra feature but the foundation of trust in the product.

**Three levels of visibility** — divided by the attention and cost the user spends.

| Level | Screen | Cost | When |
|---|---|---|---|
| **1. List thumbnail** | 320×180, every 10s | Near zero | Scanning several Desktops on the dashboard |
| **2. Live observation (READ)** | Full resolution WebRTC, no input | 1.8 vCPU of encoding, from the host reservation | Watching the agent work |
| **3. Direct operation (WRITE)** | Full resolution + input | Same as above | Intervening, setting up, correcting |

Levels 2 and 3 cost the same infrastructure. **What separates observing from operating is only the control lease.**

Moving between them is **one button, with no reconnection.** If something looks wrong while watching, the user must be able to take over immediately (§5.6 preemption).

| ID | Capability | Description | Priority |
|---|---|---|---|
| V-01 | Live screen streaming | WebRTC (fallback: WebSocket JPEG diff) | P0 |
| V-02 | **Full input control** | Mouse, keyboard, scroll, drag — indistinguishable from a local PC | P0 |
| V-03 | Preempt / release the lease | Immediate preemption from the agent, explicit release | P0 |
| V-04 | Read-only mode | Observation without the lease | P0 |
| V-05 | Clipboard sync | Two-way copy/paste between local and Desktop. **Manual by default** (sent on paste) — automatic sync would let the agent read the person's local clipboard via `clipboard.read`, so it is opt-in | P0 |
| V-06 | File drag and drop | Browser → Desktop upload and the reverse | P1 |
| V-07 | Special key passthrough | Ctrl+Alt+Del, Win/Cmd, F11, Hangul/English toggle | P1 |
| V-08 | Session recording | Action log + frame recording | P1 |
| V-09 | Embedded viewer | iframe/SDK embedding in a customer product via a signed URL | P1 |
| V-10 | Multiple observers | Several people observing (READ) at once; WRITE stays with one | P2 |
| V-11 | List thumbnails | Recent screen preview in the Desktop list (§6.3) | P1 |
| V-12 | Fallback indicator | Tell the user when WebRTC is blocked and quality has dropped | P1 |
| V-13 | Mobile viewer | Responsive layout with touch-to-mouse mapping | P2 |

**Required viewer UI**
- Who currently holds the lease (`agent is operating` / `you are operating`)
- One-click preempt and release buttons
- Because there is no way to know what the agent was about to do, an optional field to **send the agent a reason when preempting**

**One input path:** the viewer's mouse and keyboard input is **converted into the same Computer API actions** as the agent's and passes through the same queue. There is no separate input path, because that would split the audit log and make lease arbitration impossible.

**The concrete path — without this decided, the viewer cannot be built.**

```text
browser ──DataChannel──► stream gateway ──§19.5 control stream──► guest
                              │                    (source: viewer + session_id)
                              └──audit record──► Control Plane
```

| Decision | Value | Reasoning |
|---|---|---|
| Does input traverse the Control Plane | **No** | Doing so adds another round trip to §6.3's 20–50ms input budget and destroys the feel |
| Then who writes the audit record | **The gateway, asynchronously, to the Control Plane** | Auditing does not need to sit synchronously in the input path |
| Where is the lease checked | The gateway verifies the session holds `WRITE` before forwarding | The gateway subscribes to lease state |
| How does the guest tell them apart | `source: viewer` plus `session_id` on the frame | Used for audit attribution and §5.6 handover |

**Mouse motion is coalesced.** A person dragging generates 60–120 events per second; turning each into an action immediately exceeds §8.2's request cap. The gateway **coalesces coordinates to the capture tick (at most 30/s)** and the guest applies only the last one. The audit log records **one entry per input burst** — a pixel-by-pixel trace has no audit value and only inflates the log.

**Hangul input:** the viewer intercepts the browser's IME composition events (`compositionend`) and **sends the completed string as a `type` action.** Sending key by key produces double composition against the guest IME.

#### Embedding in a parent product (V-09)

**A parent product's users never see the Iapetus dashboard.** §14.1 argues that a developer who cannot watch the screen does not understand the product; for a customer embedding Iapetus, the same argument applies to *their* users, and the only surface those users ever reach is the customer's own page. `viewer_url` is therefore designed to be framed, and the rules that make framing safe are fixed here rather than discovered during integration.

| Decision | Value | Reasoning |
|---|---|---|
| Framing default | **Denied.** `Content-Security-Policy: frame-ancestors 'none'` | `viewer_url` carries a token in the query string and can hold `WRITE`. A default-open frame policy makes clickjacking against a live desktop a one-line attack |
| Opt-in | `embed_origins[]` on the project policy (§9.2) | Per project, exact-origin match. No wildcard subdomains: `*.example.com` includes whatever subdomain an attacker gets to host content on |
| Enforcement | `frame-ancestors` emitted from `embed_origins`, **and** the viewer re-checks `document.referrer` / `window.parent.origin` before enabling input | CSP alone fails open on browsers that do not enforce it; the second check degrades to READ rather than to nothing |
| Cross-origin isolation | The viewer sets no `SharedArrayBuffer` requirement | Demanding COOP/COEP would force the parent page to adopt them too, breaking most existing pages for a capability the viewer does not need |

**The parent page and the viewer talk over `postMessage`.** The parent already knows the Desktop id and can call the REST API; what it cannot get from outside the frame is *viewer-local* state — whether this user currently holds the lease, whether WebRTC fell back, whether the token hit its refresh cap. Polling the API for the first of those would also lag the thing it describes.

Every message carries `{ source: "iapetus", version: 1, type, data }`, and each side **must** verify `event.origin` before acting.

| Direction | `type` | `data` | Purpose |
|---|---|---|---|
| viewer → parent | `ready` | `desktop_id`, `session_id` | The stream is up. Before this, commands are dropped |
| viewer → parent | `control` | `level` ∈ `read`\|`write`, `holder{type,id}` | Lets the parent render its own "agent is operating" state instead of two competing indicators |
| viewer → parent | `quality` | `mode` ∈ `webrtc`\|`fallback`, `fps` | V-12. The parent may already have a place to show degradation |
| viewer → parent | `token_expiring` | `expires_in_sec`, `capped` | `capped: true` means the 8-hour `orig_iat` cap is reached (§8.1) and self-refresh will not save it. **Without this the embedded viewer simply goes black at hour eight** |
| viewer → parent | `error` | `code`, `message` | §8.9 codes, so the parent needs no second error vocabulary |
| parent → viewer | `set_token` | `token` | Hands in a freshly minted Viewer Token. The only party that can authenticate the end user is the parent, which is why the refresh cap resolves here and not in the browser |
| parent → viewer | `request_control` / `release_control` | — | Drives §7.5's one-button takeover from the parent's own chrome |
| parent → viewer | `set_chrome` | `hide[]` ∈ `toolbar`\|`status` | Lets the parent supply its own UI. **The lease indicator cannot be hidden** — a user who cannot tell whether the agent is driving is exactly the failure §7.5 exists to prevent |

**`set_token` does not widen authority.** The token is validated against the same Desktop and actor as the one the frame opened with; a token for a different Desktop is rejected rather than followed, because otherwise a compromised parent page could pivot one embedded frame across a project's desktops.


---
## 8. External Interface Specification

### 8.1 Authentication and authorization

Iapetus authenticates **two kinds of principal.** The means differ, but once authenticated their Desktop authority is identical (§5.2).

#### Credential types

| Type | Format | Held by | Lifetime | Purpose |
|---|---|---|---|---|
| **Project Key** | `sk_iap_live_...` | Customer server | Indefinite (revoked manually) | Create and delete Desktops, manage Owners, change policy |
| **Agent Token** | `at_iap_...` (JWT) | Agent runtime | 1 hour (refreshable) | Control only the named Desktops |
| **Viewer Token** | `vt_iap_...` (JWT) | Human browser | 15 minutes (auto-refresh) | Viewer stream access and input |
| **Guest Token** | `gt_iap_...` | `iapetusd` | Injected at boot, 24 hours | Guest → Control Plane outbound only |

**The Project Key is never sent to a browser or an agent process.** The customer's server exchanges it for short-lived tokens and passes those on.

```text
customer server ──[Project Key]──► POST /v1/tokens
                                    │
                    ┌───────────────┴───────────────┐
                    ▼                               ▼
              Agent Token                     Viewer Token
        (scope: specific Desktop)     (scope: specific Desktop, 15 min)
                    │                               │
                    ▼                               ▼
              agent runtime                   human browser
```

#### Token scopes

Tokens are issued **narrowed to a Desktop.** An agent must not reach another customer's Desktop, nor another Desktop in the same project.

```json
{
  "sub": "agent_123",
  "actor_type": "agent",
  "desktop_ids": ["dsk_01H8XK"],
  "scopes": ["desktop:control", "desktop:files", "desktop:shell"],
  "exp": 1755250000
}
```

| Scope | Permitted actions |
|---|---|
| `desktop:read` | screenshot, app.list, window.list, event subscription |
| `desktop:control` | Mouse, keyboard, launching applications, window manipulation |
| `desktop:files` | fs.* |
| `desktop:shell` | shell.*, system.*, app.install |
| `desktop:admin` | suspend / resume / restart / delete |
| `desktop:owners:manage` | Add and remove Owners of that Desktop; convert its type |
| `desktop:audit:read` | Read that Desktop's audit log |
| `project:manage` | Create Desktops (**including naming the initial Owners at creation**), change project policy. Project Key only |

**Human actors receive two scopes unconditionally.** Every Viewer Token issued with `actor.type: "human"` carries `desktop:owners:manage` and `desktop:audit:read`, and **the customer cannot exclude them.** An issuance request that tries is rejected with `400 CANNOT_WAIVE_HUMAN_RIGHTS`.

**Creation is the exception.** At creation there is no human Owner yet to protect, so naming the initial Owners through the `owners` parameter of `POST /v1/desktops` is possible with `project:manage`. Without it, a customer server could create a Desktop and the agent — not being an Owner — would be blocked by `NOT_OWNER` on every call, so the agent could only be attached by a human opening a viewer and clicking. The §8.4 SDK example, §14.1's 15-minute TTFA, and the unattended S3 routine would all fail.

**What is protected is *removal*, not first assignment.** Adding or removing Owners after creation requires `desktop:owners:manage`, which is human-only, so **a customer cannot detach a human Owner once one is attached.** That is precisely what §9.3 exists to preserve.

| Moment | Required authority | Reasoning |
|---|---|---|
| Naming initial Owners at creation | `project:manage` (Project Key) | There is no human Owner to protect yet |
| **Adding while zero human Owners exist** | `project:manage` (Project Key) | Bootstrap; see below |
| Adding or removing once a human Owner exists | `desktop:owners:manage` (human only) | Preventing the eviction of a human Owner is the point |

**A human who is not an Owner can still open a viewer.** `NOT_OWNER` blocks calls by non-Owner actors, but human actors get two exemptions: ① `READ` viewer access, and ② self-registration as an Owner under the bootstrap condition below. Without these, §14.1's "the human is added later" path does not work.

**Why the bootstrap exemption is necessary:** when attaching the first human to a Desktop that has only agent Owners, that person's Viewer Token carries `desktop:owners:manage` but **is still blocked by `NOT_OWNER` because they are not yet an Owner.** Without an exemption a human could never be added. So **only while the human Owner count is zero** may a Project Key add one, and the moment anyone attaches, that door closes. After that the customer can neither **add nor remove** a human.

This is the mechanism that makes §9.3's end-user protection **technical rather than contractual.** Without this rule, the right to evict an agent and the right to read the audit log would be mediated by the customer's server — and the customer is the very threat §9.3 identifies, so the protection would not hold. It is why Owner management must not live only in `project:manage`.

**Scopes are a different layer from the authority model (§7.3).** OWNER mode governs what can be done *inside* the Desktop, which is unlimited; scopes govern which APIs a given token may call. A token without `desktop:shell` cannot call the shell — but with `desktop:control` it can open a terminal application and type into it, ending up with a shell anyway. **Scopes prevent accidents; they are not a security boundary.** That limit is stated here deliberately.

#### Token signing and key management

Saying tokens are revoked by `jti` (see Revocation below) while the claim set contains no `jti` makes revocation unimplementable. The signing scheme is fixed here.

| Item | Value |
|---|---|
| Algorithm | **EdDSA (Ed25519)** |
| Key distribution | `GET /.well-known/jwks.json` (unauthenticated, 1-hour cache) |
| Rotation | Every 90 days. New and old keys run **in parallel for 30 days** before the old one is retired |
| Key selection | `kid` in the JWT header; verifiers pick from JWKS by `kid` |
| Emergency rotation | Issue a new key, refresh JWKS, retire the old key immediately. This invalidates every issued token, so it is used only as an incident procedure alongside revocation |

**Why Ed25519 rather than RS256:** signing and verification are fast, keys are short, and it avoids implementation traps such as RSA padding-selection errors. Every target language's JWT library supports it.

**Full claim set**

```json
{
  "jti": "jti_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "iss": "https://api.iapetus.dev",
  "aud": "iapetus",
  "sub": "agent_123",
  "actor_type": "agent",
  "project_id": "prj_01H8...",
  "desktop_ids": ["dsk_01H8..."],
  "scopes": ["desktop:control", "desktop:files", "desktop:shell"],
  "iat": 1755250000,
  "exp": 1755253600,
  "orig_iat": 1755250000
}
```

| Claim | Role |
|---|---|
| `jti` | **The handle revocation operates on.** The value `POST /v1/tokens/revoke` accepts |
| `orig_iat` | Time of first issuance. Used to enforce the self-refresh lifetime cap (8 hours for viewers, 24 for agents). `iat` alone resets on every refresh, which makes the cap meaningless |
| `actor_type` | When `human`, the two mandatory scopes above are attached automatically |

**Revocation requires state.** A JWT can be verified statelessly; revocation cannot. The Control Plane holds the revoked `jti` list in a **5-second TTL cache**, which is what backs the "< 5s" revocation commitment. Short token lifetimes (15 minutes to 1 hour) keep the list small.

**The Project Key is not a JWT.** `sk_iap_live_...` is an **opaque random string with 256 bits of entropy** (43 Base62 characters), verified by server-side hash comparison. It never expires, so it is unrelated to JWKS and rotation, and revoking it means deleting the record, which takes effect immediately.

**The Guest Token is a JWT but follows a separate issuance path.** Because `iapetusd` presents it on every §19.5 connection, its shape is specified here.

| Item | Value |
|---|---|
| Format | JWT, **the same Ed25519 keys and JWKS as above** |
| Claims | `jti`, `iss`, `aud: "iapetus-guest"`, `sub: {desktop_id}`, `iat`, `exp` |
| Issuance | At Desktop provisioning. Renewal is self-service **on the strength of the mTLS client certificate** |
| Lifetime | 24 hours |
| Scopes | None — the guest has no authority to call Control Plane APIs (§9.1). This token is used **only for the §19.5 stream connection** |

**An emergency key rotation invalidates every Guest Token at once.** Ten thousand desktops requesting reissue over mTLS simultaneously makes that path itself the bottleneck, and on failure they all fall to `DEGRADED`. Emergency rotation therefore **applies in hash buckets, staged over 15 minutes**, and that delay is accepted — Guest Tokens are used only for the stream connection, so the credentials that need immediate cutoff during an incident are the Agent and Viewer tokens.

#### Authenticating a human actor

A Viewer Token by itself does not identify a person. The customer authenticates its own user and embeds the result when requesting the token.

```http
POST /v1/tokens
Authorization: Bearer sk_iap_live_...

{
  "type": "viewer",
  "desktop_id": "dsk_01H8XK",
  "actor": { "type": "human", "id": "usr_kim", "display_name": "Kim Cheol-su" },
  "ttl_sec": 900,
  "control": "write"
}
```

The `control` field is **the maximum level this token may request** — it does not acquire the lease automatically. A viewer always starts at `READ`, and only becomes `WRITE` when the user presses the button and `POST /v1/sessions/{sid}/control/acquire` is called.

**Opening a viewer must never preempt the agent.** Users usually open the window "to see what is happening." If that act interrupted the agent's work, observing would itself become dangerous and §7.5's three levels of visibility would be pointless.

The response returns the token together with a **`viewer_url`**.

```json
{
  "token": "vt_iap_...",
  "expires_in": 900,
  "viewer_url": "https://viewer.iapetus.dev/d/dsk_01H8...?t=vt_iap_..."
}
```

`viewer_url` is **a complete address with the token embedded**, not a separate resource. It is valid as long as the token (15 minutes), and when the token self-refreshes the viewer keeps its connection rather than fetching a new address. It is not stored on the Desktop resource, because the value differs per actor.

This address is handed to the user's browser. The audit log's `actor.id` comes from here — **Iapetus trusts the user identifier the customer sends**, and responsibility for authenticating the end user rests with the customer. This trust boundary is stated in the contract.

#### Revocation

| Target | Method | Takes effect |
|---|---|---|
| Project Key | Revoke in the dashboard or API and reissue | Immediately |
| Agent / Viewer Token | `POST /v1/tokens/revoke` — by `jti` or by `desktop_id` (claims above) | < 5s (revocation list cache TTL) |
| Everything (incident response) | `POST /v1/projects/{id}/revoke-all` | < 5s. All sessions terminated |

Tokens are kept short-lived (15 minutes to 1 hour) so that **even a failed revocation closes the exposure window on its own.**

#### Refresh

The Project Key never reaches the browser, so a viewer must be able to refresh itself. It uses self-refresh: **presenting the still-valid token to extend it.**

```http
POST /v1/tokens/refresh
Authorization: Bearer vt_iap_...        # the still-valid token itself

→ { "token": "vt_iap_...", "expires_in": 900 }
```

| Rule | Value |
|---|---|
| Refreshable when | The token is still valid, has not been revoked, and the Desktop still exists |
| When it happens | The SDK or viewer calls automatically 5 minutes before expiry |
| **Total lifetime cap** | **8 hours** from first issuance. After that the customer server must issue a new token |
| Refresh after expiry or revocation | `401 TOKEN_EXPIRED`. The viewer switches to a re-issuance prompt |

The cap exists because unlimited self-refresh **destroys the effectiveness of revocation.** Agent Tokens follow the same rule (1-hour TTL, 24-hour total cap).

**Guest Tokens (`iapetusd`) refresh too.** A Desktop with `auto_suspend: false` can stay ACTIVE for weeks, so before the 24-hour expiry it reissues automatically on the strength of its mTLS certificate. On failure the Desktop is marked `DEGRADED`.

#### mTLS (the guest channel)

Separately from the Guest Token, `iapetusd` presents a **client certificate.** It is issued when the Desktop is provisioned and is destroyed with the Desktop. The guest is root and can steal the certificate, but all it enables is **using that Desktop's own channel** (§9.1).
### 8.2 API conventions

Fixed once here rather than repeated per endpoint. Everything below applies across `/v1`. **Left unstated, an implementer picks a value arbitrarily, and that value becomes the outage condition.**

#### Identifiers

| Resource | Prefix | Resource | Prefix |
|---|---|---|---|
| Desktop | `dsk_` | Secret | `sec_` |
| Session | `ses_` | Webhook | `whk_` |
| Owner entry | `own_` | Image | `img_` |
| Project | `prj_` | Event | `evt_` |
| Async job | `job_` | Webhook delivery | `dlv_` |
| Screenshot | `sht_` | Request trace | `req_` |
| Token id | `jti_` | | |

- Format: `{prefix}{26-character Crockford Base32 ULID}` — e.g. `dsk_01H8XK4M2N7P9Q3R5S6T7V8W9X`.
- **Opaque.** Assume no structure beyond the prefix. Do not infer ordering or creation time from an id.
- **Case-sensitive.** Exactly 30 characters, `[A-Za-z0-9_]` only, all server-generated. Every prefix is three characters, which is what makes the total fixed — that buys fixed-width database columns, aligned logs, and a parser that slices at a constant offset. A four-character prefix would silently make some ids 31, so the registry is asserted in `iapetus-proto`.
- **Exception — actor ids do not follow this rule.** `actor.id` (§8.1) is a value the customer asserts, so it is free-form UTF-8 up to 128 characters and **Iapetus never parses it.** It is recorded verbatim in the audit log.
- Window ids (`win_1`) are **guest-local identifiers** reused after a reboot. Never store them as persistent references.

#### Time

- Every timestamp is **RFC 3339 UTC with exactly three fractional digits**: `2026-08-15T09:42:13.220Z`. The `Z` is required; offset notation is not accepted.
- **It is Control Plane time, not guest time.** A guest clock can be wrong immediately after a resume (§7.4).
- **The only places Unix integer seconds appear are the JWT claims (`iat`, `exp`, `orig_iat`) and the `X-RateLimit-Reset` header.** Those follow JWT (RFC 7519) and rate-limit convention respectively, and **are never used in API body fields.** That list is the complete set of exceptions; every other time value is an RFC 3339 string.
- Durations are integer seconds and carry a `_sec` suffix (`idle_timeout_sec`).

#### Coordinates and numbers

- Screen coordinates are **integer pixels.** Sending a fraction returns `400 INVALID_COORDINATE` — nothing is rounded.
- Out-of-screen coordinates are **rejected, not clamped.** Silently correcting them prevents the agent from noticing its own error.
- `bounds` is a `{x, y, width, height}` object. Positional arrays are not used: a reader cannot tell whether `[100,100,420,720]` is x1y1x2y2 or xywh.
- `scale` is a real number where `0 < scale ≤ 1`. Upscaling is not supported.

#### Error responses

Every 4xx and 5xx uses the same envelope.

```json
{
  "error": {
    "code": "CONTROL_HELD",
    "message": "Another session holds the write lease.",
    "request_id": "req_01H8XK4M2N7P9Q3R5S6T7V8W9X",
    "details": { "holder": { "type": "agent", "id": "agent_123" } },
    "retry_after_sec": 30
  }
}
```

- `request_id` is **also returned on 2xx via the `X-Request-Id` header.** Without it a customer has no way to point at a §12.5 OpenTelemetry trace.
- `code` is the stable identifier from §8.9. Codes are additive, so **clients must handle unknown codes by HTTP status.**
- `message` is English prose for humans and is not to be parsed.

#### Listing

Control Plane lists are **cursor-based.** Offsets are not used because, with thousands of Desktops, a list that changes while you page through it duplicates and drops entries.

```http
GET /v1/desktops?limit=25&starting_after=dsk_...&status=ACTIVE&label=team:sales

{
  "data": [ ... ],
  "has_more": true,
  "next_cursor": "dsk_..."
}
```

| Parameter | Default | Max |
|---|---|---|
| `limit` | 25 | 100 |
| `starting_after` | — | Only a server-issued id is valid |

- Ordering is fixed to `created_at` descending. v1 offers no choice of sort key.
- Filters: `status`, `label`, `os`, `desktop_type`, combined with AND. Audit logs narrow by `from` and `to`; unbounded, they return **only the last 24 hours.**

**Guest-side lists are not paginated.** `fs.list`, `process.list`, `window.list`, and `app.list` have no stable snapshot in the guest to hold a cursor against. They **truncate at 1,000 entries and return `truncated: true`** instead. The agent narrows the path and calls again.

#### Caps

Nothing is unbounded.

| Subject | Cap | On exceeding |
|---|---|---|
| `act` action array | **64 actions**, 120s total | `400 BATCH_TOO_LARGE` |
| `type` text | 8,192 characters | `400 PAYLOAD_TOO_LARGE` |
| Inline file upload | 32MB. Above that a presigned URL is **mandatory** | `413` |
| Total file upload | 5GB | `413` |
| `shell.exec` output | 1MB each for stdout and stderr; the excess is cut and `truncated: true` returned | — |
| Screenshot | 4096×4096. `quality` 1–100, `format` ∈ `jpeg`\|`png`\|`webp` | `400` |
| Inline base64 screenshot | Only below 256KB; above that a URL is returned | — |
| Concurrent Sessions per Desktop | **1 WRITE + 10 READ** | `409 TOO_MANY_SESSIONS` |
| Owners per Desktop | 50 | `400` |
| Webhooks per project | 20 | `400` |
| Path length | 4,096 (Linux) / 260 (Windows default) | `400 INVALID_PATH` |

The READ session cap of 10 is **deliberately above** the six concurrent streams per host (§12.4). If the two caps matched, a caller could not tell which one it hit — with them separated, a session that attaches but finds no stream capacity gets a clear `NO_STREAM_CAPACITY`.

#### Timeouts

| Subject | Default | Max |
|---|---|---|
| `act`'s `timeout_ms` (**whole batch**) | 30,000 | 120,000 |
| `shell.exec` | 30,000 | 300,000 |
| `wait_for` | 10,000 | 120,000 |

**A timeout is not a cancellation.** `ACTION_TIMEOUT` means "we will stop waiting for a response"; **whether the action ran is unknown.** A click or a keystroke may already have landed. That is why `Idempotency-Key` is **mandatory, not optional,** on state-changing actions (§8.4). Before retrying, confirm actual state with `screenshot`.

#### Encoding

- Requests and responses are UTF-8 JSON, `Content-Type: application/json; charset=utf-8`.
- Text in a `type` action is **normalized to UTF-8 NFC** before reaching the guest. Mixed composed and decomposed Hangul otherwise causes the guest IME to split jamo (§11 internationalization, §15.2).
- Other strings are not normalized.
- **Binary travels by presigned URL as a rule.** Two exceptions are allowed where saving a round trip is worth it — uploads under 32MB and base64 screenshots under 256KB (caps above). Beyond those, a URL is mandatory.

#### Versioning

| Axis | Notation | Governs |
|---|---|---|
| Transport shape | Path `/v1` | A `/v2` is created only for breaking changes, run in parallel for 12 months |
| Behavioral change | Header `Iapetus-Version: 2026-08-15` | Additive and behavioral changes. Unset, the account's pinned version applies |

- Within `/v1` we **only add fields.** Removal, changed meaning, and newly-required fields are breaking changes.
- Clients **must ignore fields they do not recognize.**
- Deprecations carry `Deprecation` and `Sunset` headers.
- **The §19.4 daemon protocol integer is internal and is never exposed to API clients.** The two are kept unlinked so that an N-2 daemon cannot silently change the behavior of a `/v1` call. When a daemon version difference limits functionality, the Desktop is marked `DEGRADED` and that fact appears in the API response.

### 8.3 Resource schemas

`S` = server-set (ignored if the client sends it), `C` = client-set, `C!` = client-set at creation only.

#### Desktop

| Field | Type | | Default | Note |
|---|---|---|---|---|
| `id` | string | S | — | `dsk_` (§8.2) |
| `name` | string | C | — | **Unique within the project.** `^[a-z0-9][a-z0-9-]{0,62}$` |
| `project_id` | string | S | — | |
| `os` | enum | C! | `linux` | `linux` \| `windows` |
| `image` | string | C! | Default image | `img_` id or a catalog tag |
| `spec_tier` | enum | C | `standard` | See table below |
| `display` | object | C | 1920×1080@96 | `{width, height, dpi}` |
| `desktop_type` | enum | C! | `personal` | `personal` \| `shared` (§5.2). Conversion has its own endpoint |
| `privilege_mode` | enum | C | `owner` | `owner` \| `restricted` (§7.3) |
| `os_user` | object | S | — | `{name, sudo}`. Fixed by the image, not changeable |
| `owners` | array | C! | — | At creation only; afterwards via the dedicated endpoint (§8.1) |
| `status` | enum | S | — | §5.4 |
| `persistent` | bool | S | `true` | Always true in v1 |
| `idle_timeout_sec` | int | C | 900 | 60 – 86400 |
| `auto_suspend` | bool | C | `true` | |
| `labels` | map | C | `{}` | Keys and values ≤63 characters, at most 20 pairs |
| `sessions` | array | S | — | Summary of attached Sessions; the basis for idle detection (§5.4) |
| `created_at` / `last_active_at` | string | S | — | RFC 3339 |

**`spec_tier` is a fixed set of tiers, not free-form numbers.** Arbitrary combinations break the §12.4 capacity model (memory overcommit 1:1, per-tier host pools).

| `spec_tier` | vCPU | Memory | Disk | Use |
|---|---|---|---|---|
| `light` | 2 | 2GB | 20GB | Simple web automation. **Five Chrome tabs is the advised ceiling** |
| `standard` | 2 | 4GB | 40GB | Default |
| `large` | 4 | 8GB | 80GB | Windows, Excel, ERP |
| `gpu` | 4 | 8GB + GPU | 80GB | Observation-heavy workloads (§6.3) |

`name` is what `confirm_name` (deletion) and `get_or_create` compare against, so it is **compared byte-exact.**

#### Image

| Field | Type | | Note |
|---|---|---|---|
| `id` | string | S | `img_` |
| `name` / `tag` | string | C | `acme/erp-desktop:2026.08` |
| `os` | enum | C | |
| `visibility` | enum | C | `public` (catalog) \| `project` |
| `source` | object | C! | **What it is built from.** See below |
| `apps` | array | C | Preinstalled application catalog (§7.3) |
| `status` | enum | S | `BUILDING` \| `READY` \| `FAILED` |
| `size_bytes` | int | S | |

`source` is one of three shapes. **Without this field D-09 cannot be implemented**, because whether we operate a build farm is undecided.

```jsonc
{ "source": { "type": "registry", "ref": "ghcr.io/acme/erp-desktop:2026.08" } }  // customer builds and pushes
{ "source": { "type": "base_image", "base_image_id": "img_...",                  // base image + setup script
              "setup_script": "https://..." } }
{ "source": { "type": "dockerfile", "context_url": "https://..." } }             // Phase 3 and later
```

**v1 supports `registry` only.** The customer builds in their own CI, pushes to a registry, and we merely reference it. A build farm is a heavy operational burden and D-09's purpose — including organization-specific applications — is satisfied by reference alone. `base_image` is Phase 3 and `dockerfile` comes after.

Registration is `POST /v1/images`, and image validation (size, layers, `iapetusd` injection compatibility) is an async job (§8.4).

#### Policy

Project policy and Desktop overrides use **the same document shape**, and a read returns the merged result.

```json
{
  "network": { "mode": "standard", "deny_domains": [], "allow_domains": null },
  "recording": { "forced": false, "retention_days": 90, "deletion_lock": false },
  "audit_params": "digest",
  "privilege_mode": "owner",
  "approval_required": [],
  "masking": [{ "window_title_regex": "^Bank", "region": null }],
  "embed_origins": []
}
```

- `embed_origins` is **project-level only** (§7.5). A Desktop-level override would let one Desktop widen the set the project allowed, which is the wrong direction for a control that exists to bound who may frame a live session.
- **Merging replaces whole top-level keys.** There is no deep merge: partially inheriting `deny_domains` would leave nobody certain which list is in force.
- `GET /v1/desktops/{id}/policy` returns the merged result together with each key's origin (`project` \| `desktop`).

#### Secret

| Field | | Note |
|---|---|---|
| `id` | S | `sec_` + ULID. **Meaningful names do not go in the id** |
| `name` | C | Human-readable name; changeable |
| `allowed_desktop_ids` | C | Unset means the whole project is allowed, with a warning (§9.3) |
| `created_at` / `updated_at` | S | |

**The value is not readable through any API.** It is write-only and used solely by `secret.type`.

#### Webhook

| Field | | Note |
|---|---|---|
| `id` | S | `whk_` |
| `url` | C | HTTPS required |
| `events` | C | An array of §8.7 type strings. Unknown values return `400` |
| `secret` | C! | Returned once at creation and never again |
| `status` | S | `active` \| `failing` \| `disabled` |

#### Excluded from v1

| Resource | Reason |
|---|---|
| **Snapshot** | D-08 is P2 (§7.1). v1 provides automatic backups (§11) only and does not open a user-facing snapshot API |
| **Organization** | It exists only in the concept hierarchy (§5.1). v1 bills and isolates by project and has no organization API |

Both are removed from the endpoint list. **Leaving a resource referenced but unspecified is worse than leaving it out.**

### 8.4 REST API (summary)

```text
POST   /v1/desktops                      # create
   owners[]              initial Owners (project:manage, creation only)
   clone_volume_from     start from a clone of an existing volume (§12.3 ERROR recovery)
   desktop_type          personal (default) | shared (§5.2)
GET    /v1/desktops                      # list. cursor pagination + status/label/os/desktop_type filters (§8.2)
GET    /v1/desktops/{id}                 # detail
POST   /v1/desktops/{id}/suspend
POST   /v1/desktops/{id}/resume
POST   /v1/desktops/{id}/restart
DELETE /v1/desktops/{id}                 # requires confirm_name (byte-exact) → DELETING
POST   /v1/desktops/{id}/restore         # DELETING → SUSPENDED, within 24h (§10.3)

GET    /v1/desktops/{id}/owners          # Owner list (callable directly from a human viewer)
POST   /v1/desktops/{id}/owners          # add an Owner (human actors only)
DELETE /v1/desktops/{id}/owners/{oid}    # remove an Owner (human actors only)
POST   /v1/desktops/{id}/convert-to-shared  # personal → shared
   confirm_exposure: true   acknowledges that existing logins become visible to new Owners.
                            irreversible, human actors only

POST   /v1/desktops/{id}/sessions        # start a session (control: WRITE|READ)
DELETE /v1/sessions/{sid}                # end a session
POST   /v1/sessions/{sid}/heartbeat      # renew the lease
POST   /v1/sessions/{sid}/control/acquire  # request or preempt the lease
POST   /v1/sessions/{sid}/control/release  # release the lease

POST   /v1/sessions/{sid}/act            # run actions (single or batch)
GET    /v1/sessions/{sid}/screenshot
GET    /v1/sessions/{sid}/apps
POST   /v1/sessions/{sid}/apps/launch      # key or arbitrary command
POST   /v1/sessions/{sid}/apps/install
POST   /v1/sessions/{sid}/shell            # shell.exec (synchronous)
POST   /v1/sessions/{sid}/shell/spawn      # background process
GET    /v1/sessions/{sid}/processes
POST   /v1/sessions/{sid}/files/upload
GET    /v1/sessions/{sid}/files/download?path=

GET    /v1/desktops/{id}/events          # SSE: status / app / window / lease events
wss:   /v1/sessions/{sid}/control        # WebSocket control channel (§8.5)
GET    /v1/desktops/{id}/audit           # audit log (callable directly from a human viewer)
GET    /v1/images                        # image catalog
POST   /v1/images                        # register an organization image (D-09, async)
GET    /v1/jobs/{id}                     # async job status (§8.4)

GET    /v1/health                        # SLA probe (unauthenticated, §12.1)
GET    /.well-known/jwks.json            # public keys for token verification (unauthenticated, §8.1)

# --- authentication (§8.1) ---
POST   /v1/tokens                        # issue an Agent/Viewer token (Project Key required)
POST   /v1/tokens/refresh                # self-refresh before expiry (called from the browser)
POST   /v1/tokens/revoke                 # revoke by jti or by desktop_id
POST   /v1/projects/{id}/revoke-all      # incident response: terminate every session

# --- policy ---
GET    /v1/projects/{id}/policy
PUT    /v1/projects/{id}/policy          # network, recording, approvals, privilege_mode
GET    /v1/desktops/{id}/policy          # project policy merged with the Desktop override, with per-key origin
PUT    /v1/desktops/{id}/policy          # set the Desktop override (top-level key replacement)
DELETE /v1/desktops/{id}/policy          # remove the override → revert to project policy

# --- secrets ---
POST   /v1/projects/{id}/secrets         # create (value is write-only)
GET    /v1/projects/{id}/secrets         # metadata only (id, name, created_at)
PUT    /v1/projects/{id}/secrets/{id}    # replace the value
DELETE /v1/projects/{id}/secrets/{id}

# --- webhooks ---
POST   /v1/projects/{id}/webhooks        # register (url, events[], secret)
GET    /v1/projects/{id}/webhooks
DELETE /v1/projects/{id}/webhooks/{wid}
POST   /v1/projects/{id}/webhooks/{wid}/test

# --- Desktop scope (no Session required; works while SUSPENDED) ---
GET    /v1/desktops/{id}/files?path=     # read the volume directly
POST   /v1/desktops/{id}/files           # write to the volume directly
GET    /v1/desktops/{id}/files/download?path=
POST   /v1/desktops/{id}/export          # export the whole volume (§10.4)
```

**Session scope versus Desktop scope**

File operations have two paths. Choosing the wrong one wakes a SUSPENDED Desktop and costs money.

| Path | Required state | Character |
|---|---|---|
| `POST /v1/sessions/{sid}/files/upload` | `ACTIVE` + the lease | Running applications see it immediately; synchronized with the GUI flow |
| `GET /v1/desktops/{id}/files` (read) | Any state | Reads the volume directly without waking the Desktop. For retrieving results and ERROR recovery |
| `POST /v1/desktops/{id}/files` (write) | Any state (**with a constraint**) | See the rule below |

**Writing to a SUSPENDED volume, allowed naively, corrupts the filesystem.**

`SUSPENDED` is not a simple stop but a **memory snapshot.** Inside it the guest kernel's page cache, dentries, and inode state are frozen on the assumption of the disk contents at that instant. Mutating the block device from outside and then restoring the snapshot leaves the kernel's metadata disagreeing with the actual disk, producing **file corruption and journal inconsistency.**

**Decision: a write invalidates the memory snapshot.**

| State | Read | Write |
|---|---|---|
| `ACTIVE` | ✅ | ⚠️ Allowed, with `warning: "desktop_is_active"` in the response. May race a file an application has open |
| `SUSPENDED` | ✅ (crash-consistent) | ✅ **but the memory snapshot is discarded.** The next resume becomes a cold boot and running processes are lost |
| `ERROR` | ✅ | ✅ (the snapshot is already moot) |

A SUSPENDED write must set `acknowledge_snapshot_loss: true`. Without it the request is rejected with `409 SNAPSHOT_WOULD_BE_DISCARDED` so the caller confirms intent. **Login sessions and files live on the volume and survive** — what is lost is process state alone.

#### Webhooks

Events are pushed to the customer's server rather than polled. This is what lets a scheduled routine (S3) run without the agent runtime holding a permanent connection.

```http
POST https://customer.example.com/hooks/iapetus
X-Iapetus-Delivery: dlv_01H8XK4M2N7P9Q3R5S6T7V8W9X
X-Iapetus-Timestamp: 1755250933
X-Iapetus-Signature: sha256=...

{ "id": "evt_01H8...", "type": "control.revoked", "created_at": "2026-08-15T09:42:13.220Z",
  "project_id": "prj_01H8...", "desktop_id": "dsk_01H8...", "data": { } }
```

- **The body is the §8.7 event envelope verbatim.** The type and event id are not header-only, because a receiver that stores just the body would lose both. The `X-Iapetus-Delivery` header distinguishes retry attempts — the same `id` can arrive several times.
- Signature: `HMAC-SHA256(secret, X-Iapetus-Timestamp + "." + raw_body)`. The receiver verifies against **the raw bytes.**
- Replay protection: reject if `X-Iapetus-Timestamp` (Unix seconds) is more than five minutes off.
- Retry: anything other than 2xx is retried with exponential backoff up to six times (about an hour). After that `webhook.failed` goes to SSE and the dashboard (§8.7).
- **No ordering guarantee.** Receivers sort by `created_at` and deduplicate on `id`.

#### Rate limits and quotas

Two layers. **Rate limits govern call frequency; quotas govern total resources.**

| Subject | Default limit | On exceeding |
|---|---|---|
| Control actions (`act`, `click`, `type`) | 20 req/s per Desktop | `429 RATE_LIMITED` + `Retry-After` |
| Continuous input (`mouse.move`, `scroll`) | **Not subject to the request-rate limit.** Separately capped at 60 events/s | Excess is coalesced (§7.5) |
| `screenshot` | 5 req/s per Desktop | 429 |
| Desktop creation | 10 req/min per project | 429 |
| Token issuance | 100 req/min per project | 429 |
| Other management APIs | 50 req/s per project | 429 |

| Quota | Free | Pro | Enterprise |
|---|---|---|---|
| Concurrent ACTIVE Desktops | 1 | 25 | Negotiated |
| Total Desktops (including SUSPENDED) | 2 | 200 | Negotiated |
| Monthly compute hours | 10h | 1,000h | Negotiated |
| Total volume | 20GB | 2TB | Negotiated |
| Audit log retention | 30 days | 90 days | 365 days |

Exceeding a quota returns `429 QUOTA_EXCEEDED` and **does not terminate running Desktops.** Only new creation is blocked, with notice via the dashboard and webhooks — an agent's in-flight work being killed by a quota is the larger harm.

Rate-limited responses always carry these headers.
```text
X-RateLimit-Limit: 20
X-RateLimit-Remaining: 3
X-RateLimit-Reset: 1755250000
Retry-After: 1
```

#### Synchronous versus asynchronous

Anything taking more than a few seconds is handled as an **async job resource.** Leaving this unstated means an SDK author guesses from the first call onward.

| Async (`202` + `job_`) | Synchronous |
|---|---|
| Desktop create / resume / restart / delete | Everything else |
| Image registration and validation | |
| `app.install` | |
| Volume export | |

```jsonc
// POST /v1/desktops → 202
{ "job_id": "job_01H8...", "resource_id": "dsk_01H8...", "status": "running" }

// GET /v1/jobs/job_01H8...
{
  "id": "job_01H8...", "type": "desktop.create", "status": "succeeded",
  "resource_id": "dsk_01H8...", "started_at": "...", "finished_at": "...",
  "error": null
}
```

`status` ∈ `running` \| `succeeded` \| `failed`. Job records are retained for **7 days** — a delete job tracks the 24-hour `DELETING` grace period, so 24-hour retention would drop the record before the job ends.
The SDK's `create()` polls to completion by default and can return immediately with `wait=False`.

#### Control lease endpoints

The product's signature feature, so the bodies are specified.

```jsonc
// POST /v1/sessions/{sid}/control/acquire
{ "note": "Taking over briefly to enter a 2FA code" }   // optional, <=200 chars, delivered to the agent

// 200 — returns the full Session object
{
  "id": "ses_9f2", "desktop_id": "dsk_01H8XK",
  "actor": { "type": "human", "id": "usr_kim" },
  "control": "WRITE",
  "heartbeat_interval_sec": 30,          // server-assigned
  "lease_expires_at": "2026-08-15T09:47:00.000Z",
  "started_at": "2026-08-15T09:42:00.000Z"
}
// 409 CONTROL_HELD — a human holds it and the requester is an agent (§5.6)
```

```jsonc
// POST /v1/sessions/{sid}/heartbeat  →  200
{ "lease_expires_at": "2026-08-15T09:47:30.000Z" }
// 409 CONTROL_LOST — already reclaimed; call acquire again to take it back
```

**Lease lifetime is `heartbeat_interval_sec × 3`** — 90 seconds at the 30-second default. It is the same value as §5.6's "reclaim after three missed intervals," derived here so the two places cannot drift apart.

`POST /v1/sessions/{sid}/control/release` has no body and returns `204`.

#### Owner management

```jsonc
// POST /v1/desktops/{id}/owners
{ "type": "agent", "id": "agent_456" }
// 201 → { "owner_id": "own_...", "type": "agent", "id": "agent_456", "added_at": "..." }
// 403 HUMAN_ONLY           — called by an agent
// 403 BOOTSTRAP_CLOSED     — called with a Project Key when a human Owner already exists
// 409 REQUIRES_SHARED_TYPE — adding a second human to a personal Desktop
```

#### Creating a session

```jsonc
// POST /v1/desktops/{id}/sessions
{ "control": "read" }     // read (default) | write — even write does not acquire immediately (§8.1)
// 201 → Session object (same shape as the acquire response above)
// 409 DESKTOP_NOT_READY  — SUSPENDED; call resume first
```

**Action request example**

```http
POST /v1/sessions/ses_9f2/act
Authorization: Bearer sk_iap_...
Idempotency-Key: 01H8XK-0007

{
  "actions": [
    { "type": "type", "text": "Cocso" },
    { "type": "key", "keys": "Enter" }
  ],
  "return_screenshot": true,
  "timeout_ms": 10000
}
```

**Response**

```json
{
  "session_id": "ses_9f2",
  "results": [
    { "type": "type", "ok": true, "elapsed_ms": 412 },
    { "type": "key", "ok": true, "elapsed_ms": 38 }
  ],
  "screenshot": {
    "url": "https://cdn.iapetus.dev/shot/...",
    "width": 1920, "height": 1080,
    "taken_at": "2026-08-15T09:42:13.220Z"
  },
  "desktop_status": "ACTIVE"
}
```

> The 412ms on `type` is the accumulated per-character delay and is normal. KPIs are separated by action class (§2.2).

#### Idempotency (`Idempotency-Key`)

GUI actions are not idempotent by nature: sending `click` twice presses twice. Retrying after a network timeout is **the most common failure mode**, so it is defined as follows.

- The server **caches the response** per `Idempotency-Key`, TTL 24 hours.
- A repeat request with the same key **does not re-execute the action** and returns the cached response.
- If the first request is still running, `409 REQUEST_IN_FLIGHT` is returned; the client backs off and re-reads with the same key.
**Key format and scope**

| Item | Value |
|---|---|
| Generated by | The client (the SDK does it automatically) |
| Format | `[A-Za-z0-9_-]{1,255}` |
| **Scope** | **`(project_id, session_id, endpoint)`** |
| TTL | 24 hours |

**Scoping to the session rather than globally is the point.** Globally scoped, two different tenants using the same key string would have one receive the other's cached response. Now that the key is mandatory, that is not a matter of style but **of isolation.**

**Two paths that do not require a key**

| Path | Reason |
|---|---|
| WebSocket control channel (§8.5) | Frames are ordered per connection and never retransmitted (§19.5). There is no notion of retry, so a key is meaningless |
| Viewer input (§7.5) | A human click is not a retryable RPC. The gateway does not retransmit input; if it is lost, the person clicks again |

So **idempotency keys apply only to the REST action path.** Without stating that boundary, an implementer would enforce keys on WebSocket and viewer input too and break both.

- **The key is mandatory on state-changing actions.** Without it, `400 IDEMPOTENCY_KEY_REQUIRED`. Since `ACTION_TIMEOUT` means "execution unknown" (§8.2), a keyless retry structurally permits double execution. Observation actions (`screenshot`, `app.list`, and similar) do not require one.
- The SDK attaches it automatically, so users never handle it directly.

Without this contract, "timeout → retry → the message is sent twice" happens by construction.

### 8.5 WebSocket control channel

For interactive loops that need minimal latency. It exchanges the same action schema as REST, frame by frame.

**Within one connection, frames execute in arrival order (FIFO).** Pipelining is permitted but reordering is not, and the in-flight depth is 8. The guarantee derives from the §19.5 guest stream preserving order. There is no ordering guarantee *between* connections — if order matters, use the same connection or group the actions into an `act`.

**WebSocket frames carry no idempotency key** (§8.4), because there is no retransmission.

```text
wss://api.iapetus.dev/v1/sessions/{sid}/control
  → { "id": 1, "type": "mouse.click", "x": 640, "y": 120 }   // id is uint64
  ← { "id": 1, "ok": true, "elapsed_ms": 22 }
  ← { "event": "window.opened", "window": {...} }      # server push
```

### 8.6 SDK

```python
from iapetus import Iapetus

client = Iapetus(api_key="sk_iap_...")

desktop = client.desktops.get_or_create(
    name="sales-agent-desktop",
    os="windows",
    owners=[                                  # settable at creation only (§8.1)
        {"type": "agent", "id": "agent_123"},
        {"type": "human", "id": "usr_kim"},
    ],
    labels={"team": "sales"},
)
# If an existing Desktop is returned, owners is ignored — changing Owners after
# creation is human-only, so the SDK does not apply it on your behalf.
# Check desktop.owners if you need to detect a mismatch.

with desktop.session() as c:          # acquires the lease, heartbeats automatically
    # a catalog application
    c.launch_app("acme_messenger", wait_for_window=True)
    shot = c.screenshot()
    c.click(320, 210)
    c.type("Hello")
    c.key("Enter")

    # anything not in the catalog runs just the same (OWNER mode)
    c.launch_app(command=r"C:\Program Files\SomeERP\erp.exe", args=["--kiosk"])

    # administrator shell and installation
    r = c.shell("winget install --id Notepad++.Notepad++ -e", elevated=True)
    assert r.exit_code == 0
# Session ends. The Desktop stays ACTIVE and auto-suspends once idle_timeout
# elapses with no sessions attached.
```

### 8.7 Event stream

Pushed over `GET /v1/desktops/{id}/events` (SSE) and the WebSocket control channel. **An agent must not learn it lost the lease only on its next action call.**

Every event shares the envelope below; only `data` differs by type. **SSE and webhooks use the same envelope** so a receiver never needs a second parser for the other transport.

```json
{
  "id": "evt_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "type": "control.revoked",
  "created_at": "2026-08-15T09:42:13.220Z",
  "project_id": "prj_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "desktop_id": "dsk_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "data": {}
}
```

Webhook deliveries carry **this same body.** Putting the delivery id and type in headers only would lose both for a receiver that stores just the body.

| Event | `data` | When |
|---|---|---|
| `control.granted` | `session_id`, `actor{type,id}` | The lease is acquired |
| `control.revoked` | `session_id`, `by_actor{type,id}`, `reason` ∈ `preempted`\|`lease_expired`\|`session_closed`, `note?` | **A human preempted**, or the lease expired |
| `control.requested` | `by_actor{type,id}` | An agent requested while a human holds it |
| `desktop.status` | `from`, `to`, `reason?` | State transition (§5.4) |
| `desktop.error` | `reason`, `recoverable`, `data_recoverable` — whether the volume can still be retrieved (§12.3) | Entering an unrecoverable state |
| `job.finished` | `job_id`, `type`, `status`, `error?` | An async job completed (§8.4) |
| `quota.exceeded` | `quota`, `limit`, `current` | A quota was reached (blocks new creation only) |
| `window.opened` / `window.closed` | `window{id,title,bounds}` | A window changed |
| `app.exited` | `key?`, `pid`, `exit_code`, `crashed` | An application exited |
| `viewer.joined` / `viewer.left` | `actor{type,id}`, `control` | A viewer attached or left |

`webhook.failed` is **SSE and dashboard only** and is never sent by webhook. Announcing a webhook failure by webhook is circular.

**Delivery is at-least-once and unordered.** Receivers must deduplicate on `id` and sort by `created_at`. The same event arriving over both SSE and webhook is normal.

**SSE reconnection:** each event carries an SSE `id:` field, and a client reconnecting with `Last-Event-ID` **receives anything missed within the last 24 hours.** A `:keepalive` comment frame every 15 seconds prevents proxies from dropping idle connections.

Recommended agent behaviour on `control.revoked`: stop the work in progress and wait until the human is finished. The screen may already have changed, so **always take a fresh screenshot before resuming.**

### 8.8 MCP server

The Computer API is exposed as MCP tools for agent runtimes that speak MCP.

| MCP tool | Maps to |
|---|---|
| `desktop_screenshot` | `screenshot` |
| `desktop_click` | `mouse.click` |
| `desktop_type_text` | `type` — suffixed `_text` so it does not collide with the `desktop_type` field (§5.2) |
| `desktop_key` | `key` |
| `desktop_scroll` | `scroll` |
| `desktop_launch_app` | `app.launch` |
| `desktop_list_apps` | `app.list` |
| `desktop_wait` | `wait_for` |

### 8.9 Error codes

| Code | HTTP | Meaning |
|---|---|---|
| `DESKTOP_NOT_FOUND` | 404 | No such Desktop |
| `DESKTOP_NOT_READY` | 409 | PROVISIONING or SUSPENDED |
| `CONTROL_LOST` | 409 | Lease lost (human preemption, lease expiry) |
| `CONTROL_HELD` | 409 | Another session holds the lease. Includes `holder` and `retry_after_sec` |
| `REQUEST_IN_FLIGHT` | 409 | A request with the same `Idempotency-Key` is still running |
| `IDEMPOTENCY_KEY_REQUIRED` | 400 | Missing idempotency key on a state-changing action (§8.4) |
| `INVALID_COORDINATE` | 400 | Fractional or out-of-screen coordinate (§8.2) |
| `BATCH_TOO_LARGE` | 400 | More than 64 actions in `act` |
| `PAYLOAD_TOO_LARGE` | 400/413 | Text or upload cap exceeded (§8.2) |
| `INVALID_PATH` | 400 | Path length or format violation |
| `TOO_MANY_SESSIONS` | 409 | Per-Desktop session cap exceeded |
| `NOT_OWNER` | 403 | Call by a non-Owner actor. **Exception:** a human actor may open a viewer and self-register under bootstrap even without being an Owner (§8.1) |
| `HUMAN_ONLY` | 403 | An agent attempted Owner management or type conversion |
| `BOOTSTRAP_CLOSED` | 403 | A Project Key attempted Owner changes on a Desktop that already has a human Owner (§8.1) |
| `CANNOT_WAIVE_HUMAN_RIGHTS` | 400 | An attempt to exclude `desktop:owners:manage` / `desktop:audit:read` when issuing a human token (§8.1) |
| `REQUIRES_SHARED_TYPE` | 409 | Adding a second human Owner to a `personal` Desktop (§5.2). Call `convert-to-shared` first |
| `APP_NOT_ALLOWED` | 403 | Occurs only in `restricted` mode: an application outside the allowlist |
| `EXEC_FAILED` | 400 | Executable missing or abnormal exit (not an authority problem) |
| `UNSUPPORTED_ON_OS` | 400 | Action unsupported on this OS |
| `ACTION_TIMEOUT` | 504 | No response from the guest |
| `RATE_LIMITED` | 429 | Call frequency exceeded. Includes `Retry-After` (§8.4) |
| `QUOTA_EXCEEDED` | 429 | Desktop count or hour limit exceeded. Running work is not interrupted |
| `TOKEN_EXPIRED` | 401 | Token expired or revoked, including exceeding the refresh cap (8h/24h) |
| `SNAPSHOT_WOULD_BE_DISCARDED` | 409 | SUSPENDED volume write without `acknowledge_snapshot_loss` (§8.4) |
| `NO_RESUME_CAPACITY` | 503 | No free memory on the affinity host. Offers snapshot discard and cold boot |
| `NO_STREAM_CAPACITY` | 503 | Host encoding reservation exhausted; concurrent observation cap reached (§12.4) |
| `POLICY_BLOCKED` | 403 | Blocked by policy (domain, shell, or file) |

---

## 9. Security and Policy

### 9.1 Isolation — the trust boundary is outside the Desktop

An agent holding root inside the guest means **the guest OS cannot be used as a security boundary.** Isolation is therefore enforced entirely outside the guest.

| Boundary | Mechanism |
|---|---|
| Kernel isolation | One Desktop = one microVM (**Firecracker**, alternatively Cloud Hypervisor). Shared-kernel containers are **forbidden** for multi-tenancy |
| Network | A dedicated network namespace per Desktop. Egress-only by default, inbound blocked entirely |
| Neighbour access | No L2/L3 traffic between Desktops. Metadata endpoint (169.254.169.254) blocked |
| Host credentials | No host or cloud credentials are ever injected into a guest |
| Control channel | Guest → Control Plane outbound mTLS only. The guest has no authority to call Control Plane APIs |
| Resources | Hard quotas on CPU, memory, disk, and network. Defends against fork bombs and disk exhaustion |
| Volumes | A dedicated encrypted volume per Desktop; no access to another's |

#### Network control is only effective outside the guest

Proxy settings inside the guest (`http_proxy`, browser configuration, `/etc/hosts`) are **not controls.** Both the human and the agent are root and can undo all of them. Real enforcement happens only at the layers below.

```text
Desktop (guest)  ── whatever is configured here has no force
      │
      ▼
host network namespace / vNIC
      │
      ├─ ① default policy: per tier (standard = allow + denylist / hardened = DROP + allowlist)
      ├─ ② DNS forced: DNAT 53/853 to the platform resolver; direct external DNS blocked
      ├─ ③ domain filter: TLS SNI inspection (no decryption). Violations reset the connection
      ├─ ④ direct-IP block: reject new connections to IPs never seen in a DNS answer
      ├─ ⑤ private/metadata ranges blocked: 10/8, 172.16/12, 192.168/16, 169.254.169.254
      │     (projects with corporate connectivity may except specified ranges; metadata never)
      └─ ⑥ anomaly detection: bandwidth spikes, port scans, known mining-pool signatures
```

**The default is open.** On the standard tier egress is **allowed by default** and only known malicious and mining destinations are blocked, because §14.1's onboarding — open Chrome and search — has to succeed on the first attempt. DROP-by-default is enabled **only in hardened mode** (enterprise and regulated), where the customer registers allowed domains. The "unrestricted" entry in §9.2's policy table refers to this standard-tier default.

**Why SNI inspection and not MITM decryption:** decrypting requires planting a CA in the guest, which would expose the user's banking and internal system traffic in plaintext. That collides directly with §10.2's data-minimization principle. SNI alone is sufficient for domain-level allow/deny, and when ESNI/ECH spreads it is replaced by the DNS-to-IP correlation in ④.

**The DoH/DoT problem:** Chrome sends DNS-over-HTTPS on port 443 by default, bypassing the DNAT in ②. The platform resolver then never sees the query, so ④ misclassifies every connection as direct-IP. Turning DoH off inside the guest is not a control, because the guest is root.

**Response:** in hardened mode, **block the IP ranges of known public DoH resolvers at ①** (browsers fall back to system DNS automatically) and enable ④ only on top of that. The standard tier does not enable ④ and stops at ③ (SNI). When ECH spreads, SNI is hidden too, and at that point **domain-level control is abandoned in favour of destination IP reputation and traffic anomaly detection.** That transition is acknowledged in advance — domain filtering is not a permanent control.

**Stated limit:** a sophisticated attacker can tunnel through an allowed domain, for instance a covert channel over a permitted CDN. Network control exists **to deter abuse and prevent accidents**, not to stop a determined insider. Where that level is required, use full egress denial with a corporate PrivateLink-only configuration.

**`iapetusd` itself is not trusted either.** Both humans and agents are guest root, so we assume the daemon can be tampered with or bypassed. Quotas, network, and audit are all enforced at the hypervisor and host level.

### 9.2 Policy engine (per project)

**There are no in-guest constraints by default.** Policy applies mainly at the Desktop boundary.

| Policy | Enforcement point | Default |
|---|---|---|
| Network domain allow/deny | Host egress proxy | Unrestricted |
| Corporate network connectivity | VPN / PrivateLink | Off |
| File export (`fs.download`) | Control Plane | Allowed |
| Human approval for high-risk actions | Control Plane | Off |
| Forced session recording | Control Plane | Off |
| `privilege_mode` | Guest (opt-in) | `owner` (unconstrained) |
| `embed_origins[]` — origins allowed to frame the viewer (§7.5) | Control Plane (CSP `frame-ancestors`) | Empty (framing denied) |

### 9.3 Handling credentials

**First recommendation: the human signs in personally.** This is the largest practical benefit of the co-ownership model (S5). When a person connects through the viewer and signs into a messenger or internal system directly, **the credentials reach neither the agent nor Iapetus.** The agent only uses the already-authenticated session. 2FA and CAPTCHAs resolve themselves.
`secret.type` is used only where automatic sign-in is genuinely required.
```json
{ "type": "secret.type", "secret_ref": "sec_erp_password" }
```
- The audit log records only `secret_ref`; the value is never written.
- **Secrets are bound to Desktops.** Scoped only to a project, any Desktop in that project could type any secret, and the "Desktop as trust unit" principle below would not be enforced by the API. `allowed_desktop_ids` is set at creation, and leaving it unset raises a warning that the whole project is permitted.
- Screenshot masking rules (blurring a designated region) are available.

**Do not misread what `secret.type` protects.**

| Stops | Does not stop |
|---|---|
| The plaintext entering the agent's LLM context | The agent recovering it from guest memory or disk via `shell.exec` |
| The plaintext appearing in audit logs or screenshots | The agent installing a keylogger |

Under OWNER mode the agent is root. `secret.type` is **a hygiene measure against accidental exposure, not a defence against a malicious agent.** If the agent's code cannot be trusted, credentials should not be placed on that Desktop at all.

#### Protecting the end user (the three-party relationship)

The Desktop holds the end user's login sessions, the agent's code is written by the customer, and their authority is identical. In other words **the customer's code can do anything with the end user's accounts.** Technology cannot prevent this — preventing it would dismantle the product — so the response is **transparency and control.**

| Mechanism | Content | Nature |
|---|---|---|
| **Notice and consent obligation** | The customer must tell the end user which agents are attached to this Desktop with what authority, and obtain consent | Contractual (DPA) |
| **Owner visibility** | The current Owner list and agent identifiers are always visible in the viewer. `GET /v1/desktops/{id}/owners` | Technical |
| **Right to evict an agent** | A human Owner may remove an agent Owner at any time; an agent cannot remove a human (§5.2). Called **without going through the customer's server**, using the `desktop:owners:manage` scope granted automatically to Viewer Tokens | Technical |
| **Right to read the audit log** | Read your own Desktop's audit log directly from the viewer. `desktop:audit:read` is granted automatically, so the customer cannot hide it by withholding the scope | Technical |
| **Right to observe at any time** | Connect through the viewer whenever you want and see what the agent is doing (§7.5) | Technical |

**What this layer stops is scope stripping, not identity forgery.** Per §8.1 the actor identifier is a value the customer asserts, so a customer can mint a token with `actor: {type: "human", id: "…"}`, receive both scopes, and remove the genuine human Owner. The rights above are therefore **guaranteed to the legitimate user but not exclusive to them.** The identity boundary remains the contractual trust of §8.1, and this limit is tracked as R-07.

**One of §5.2's two asymmetries — "only humans change Owners" — earns its place here.** Without that rule a customer's agent could remove the human Owner and seize the Desktop. The constraint was designed for exactly this situation.

**A Desktop is the unit of credential trust.** Credentials of differing trust levels belong on **different Desktops.** Not putting a personal messenger and a corporate ERP on the same Desktop is the recommended pattern.

### 9.4 Audit and traceability

Because humans and agents share one OS account (§5.2), **who did something is proven by the control session, not by the OS.** Every **API action** is recorded in an immutable audit log including the actor.

**The audit boundary is stated precisely.** Only what passes through the Control Plane is recorded. The later behaviour of a background process started by `shell.spawn`, what an application does on its own inside the guest, and the internals of a command a person runs from a terminal **do not appear in the API log.** Under OWNER mode that is a limit of principle. Regulated customers who need full behavioural tracing are directed to session recording (§10.6) plus an in-guest audit agent, and we do not claim the API audit log alone suffices.

```json
{
  "id": "evt_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "created_at": "2026-08-15T09:42:13.220Z",
  "desktop_id": "dsk_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "session_id": "ses_01H8XK4M2N7P9Q3R5S6T7V8W9X",
  "actor": { "type": "agent", "id": "agent_123" },
  "action": "type",
  "params": { "text": "Hello" },
  "params_digest": "sha256:...",
  "result": "ok",
  "screenshot_ref": "shot_88a1"
}
```

**Raw-parameter policy (`audit_params`)**

| Value | Behaviour | Use |
|---|---|---|
| `full` | Stores `params` verbatim | Regulatory and audit needs; what was typed can be reconstructed |
| `digest` (default) | Stores only `params_digest` | Privacy first; supports tamper verification only |
| `off` | Action type only | Minimal recording |

The value of a `secret.type` is never recorded in any mode.

Lease handovers (`control.acquired` / `control.revoked`) are also audit events. The moment a human preempted an agent is the key clue in any post-hoc analysis.

Retention is set per plan (30 / 90 / 365 days).

### 9.5 Abuse prevention

- Quotas on Desktop count, concurrent execution, and monthly compute hours.
- Detection and blocking of anomalous network traffic (scraping and spam patterns).
- Terms of service: no bulk account creation, no CAPTCHA-bypass services, no unauthorized operation of other people's accounts.

---

## 10. Compliance and Data Lifecycle

### 10.1 The nature of data that accumulates on a Desktop

Iapetus **stores personal data by construction.** Rather than denying that, the design accounts for it.

| Data | Where | Personal data | Control |
|---|---|---|---|
| Application login sessions and cookies | Desktop volume | ✅ (authentication material) | Volume encryption, Desktop isolation |
| Messenger conversation content | Desktop volume | ✅ (may include third parties) | Controlled by the customer; we do not read it |
| Screenshots and session recordings | Object storage | ✅ (whatever is on screen is captured) | Retention limits, masking, off by default |
| Audit log `params` | Log store | ✅ (when `audit_params: full`) | `digest` by default |
| Account and billing information | Control Plane DB | ✅ | Standard handling |

**Role separation:** Iapetus is in principle a **processor.** The customer decides what data goes onto a Desktop, and we store and process it on their instruction. This distinction is stated in the DPA, which also fixes that notice and consent toward the end user are the customer's obligation.

**What we do not do (explicit commitments)**
- We do not read the contents of a Desktop volume. Even for support this requires **the customer's explicit approval and time-limited access.**
- We do not use screen or input data to train models.
- We do not retain screenshots long-term by default (deleted after 24 hours; only those referenced by an audit record follow the retention period).

### 10.2 Data minimization and retention

| Data | Default retention | Maximum | Configured by |
|---|---|---|---|
| Screenshots (for action responses) | 24 hours | 90 days | Project policy |
| Session recordings | Not stored (off by default) | 365 days | Explicit opt-in |
| Audit logs | 30 days | 365 days | Plan |
| Desktop volume | Indefinite (customer-owned) | — | Customer deletes |
| Volume backup snapshots | 7 days | 30 days | Plan |

**Masking sensitive screens:** policy can designate a coordinate region or a window-title regex to be obscured. Masking is applied **in the guest Frame Source, before encoding**, so screenshots, recordings, and the live stream are all covered identically (§10.6). During `secret.type` capture is not stopped — only the input region is covered, because stopping capture freezes the screen of anyone watching and looks like an outage.

### 10.3 Deletion (right to erasure)

```text
DELETE /v1/desktops/{id}  (confirm_name required)
   │
   ├─ immediately: transition to `DELETING` (§5.4). Desktop stopped, control APIs blocked,
   │               hidden from lists by default
   ├─ immediately: key moved to an isolated store and all access paths cut (logical disablement)
   ├─ 24 hours: recovery grace. `POST /v1/desktops/{id}/restore` returns it to SUSPENDED
   ├─ T+24h: encryption key destroyed (crypto-shredding) ← decryption impossible from here
   ├─ within 7 days: volume physically deleted, along with backup snapshots encrypted under the same key
   └─ within 30 days: object storage (screenshots, recordings, thumbnails) fully deleted
```

- **What "immediate deletion" precisely means:** destroying the key at T+0 makes a recovery request at T+12h impossible to honour. Both cannot be promised, so **the recovery grace takes priority**: access is cut at T+0 (logical disablement) and **the key is destroyed at T+24h.** We do not use "destroyed immediately" as marketing copy.
- **Backups are encrypted under the same key,** so destroying it disables them as well. A backup under a separate key would invalidate the crypto-shredding claim.
- For customers who do not want the 24-hour grace, `immediate_key_destruction: true` is available. The API explicitly warns that recovery becomes impossible.
- On account termination the same procedure applies to every Desktop, completing within 30 days, after which a **certificate of deletion** is issued.
- Audit logs can conflict with legal retention obligations, so they are **anonymized** rather than deleted. However, under `audit_params: full` the `params` field contains **third-party personal data** such as message bodies and names, so erasing the actor id is not sufficient. In that mode the entire `params` field is destroyed and only `params_digest` remains.

### 10.4 Portability

| Subject | Method |
|---|---|
| Entire Desktop volume | `POST /v1/desktops/{id}/export` → an encrypted tar/qcow2 image via presigned URL (valid 7 days) |
| Individual files | `GET /v1/desktops/{id}/files/download` (works while SUSPENDED) |
| Audit logs | JSONL export |
| Configuration and policy | JSON export |

**Anti-lock-in:** volumes export in standard image formats (qcow2/tar), so a customer can boot them on their own infrastructure as-is. The existence of the self-host path (§19.2) is itself a balance-of-power mechanism.

### 10.5 Target certifications

| Certification | Timing | Note |
|---|---|---|
| Internal PIPA compliance programme | Phase 2 | DPA template, privacy policy, processor disclosure |
| **ISMS-P** | Phase 3–4 | Effectively a precondition for Korean enterprise sales |
| **SOC 2 Type II** | Phase 4 | For overseas customers. Requires at least a six-month observation window |
| CSAP (cloud security assurance) | On entering public sector | Only if we decide to pursue the public market |

**Caution:** certification takes six to twelve months at minimum, and audit logs, access control, and change-management evidence **must have been accumulating since Phase 1.** None of it can be produced retroactively, so logging and change management are designed against certification requirements from the start.

### 10.6 Regulated industries

| Requirement | Response |
|---|---|
| Financial-sector network separation | Self-host deployment (§19.2) with PrivateLink-only egress |
| No cross-border data transfer | Region pinning (ap-northeast-2), backups in the same region. Windows pinned to Azure Korea Central |
| Mandatory full session recording | Forced recording via policy (**cost and constraints below**) |
| Access approval workflow | Human approval for high-risk actions (§9.2) |

**The real cost of forced recording — stated rather than hidden**

Forced recording collides directly with three of §6.3's optimizations, and the customer pays for it.

| Collision | Consequence |
|---|---|
| "No viewers, no encoder" | **Does not hold.** Encoding continues with no observer, costing ~1.8 vCPU per Desktop permanently |
| "Streaming is not billed separately" (§13) | **An exception.** The forced-recording tier carries a higher compute rate, disclosed separately |
| "Mux without re-encoding" | **Holds.** Masking is applied before encoding (guest Frame Source), so the encoded stream is never touched again |
| §12.4 per-tier density | Concentrating forced-recording tenants exhausts the encoding reservation first, so the count falls below the density table. **Separate host pool** |

**A "cannot be deleted" mode is not offered, because it conflicts with §10.3's deletion guarantee.** Instead we provide **a deletion lock for the retention period** — deletion requests are refused for, say, 365 days, after which destruction is automatic. That is the only shape that satisfies a legal retention obligation and the right to erasure at once.

**`secret.type` while someone is watching live:** stopping capture freezes the viewer's screen and looks like an outage. Instead **the input region is covered with a black rectangle in the guest Frame Source before encoding**, and the viewer shows a "sensitive input in progress" badge.

**Masking must happen in the guest, before encoding.** The gateway is an SFU that only replicates RTP (§6.3), so changing pixels there would require decode and re-encode — reviving exactly the gateway transcoding §6.3 rejected at "cost × number of viewers." Doing it in the guest means ① the cost is drawing a rectangle into an RGBA buffer, effectively zero, ② encoding still happens once, and ③ **the guest is the only component that knows when `secret.type` is running.** Since screenshots, recording, and the live stream all read the same Frame Source, **masking in one place covers all three at once.**

---

## 11. Non-Functional Requirements

| Item | Requirement |
|---|---|
| Availability | Control Plane 99.9%; Desktop runtime Linux 99.5% / Windows 99.0% (§12.1) |
| Scalability | 10,000 concurrent Desktops per region |
| Latency | Computer API p95 < 300ms (same region) |
| Data retention | Daily volume snapshot, kept 7 days |
| Regions | Linux: own infrastructure in ap-northeast-2 (Seoul), plus us-east and eu-west in v2. **Windows: Azure Korea Central** (§19.3). Latency and availability are measured separately for the two |
| Observability | Per-Desktop CPU/MEM/FPS/action-latency metrics and action traces (§12.5) |
| Internationalization | **Hangul IME input is required** on both guest and viewer. Guest locale and time zone configurable. UI copy in ko/en |
| Accessibility | Viewer fully operable by keyboard, screen-reader labels, WCAG AA colour contrast |
| Browser support | Current Chrome/Edge/Safari/Firefox. WebRTC and H.264 decoding required |
| Compatibility | `/v1` adds fields only. Behavioural changes go through the `Iapetus-Version` date header. Only breaking changes create `/v2`, run in parallel for 12 months (§8.2) |
| Documentation | OpenAPI 3.1 generated automatically (§19.1), SDK reference, per-scenario guides |

---
## 12. Operations, SLA, and Incident Response

### 12.1 SLA definitions

| Item | Target | Measurement |
|---|---|---|
| Control Plane availability | 99.9% | An external probe calls `GET /v1/health` every minute; 5xx or no response counts as a failure |
| Desktop runtime availability (Linux) | 99.5% | Success rate of control actions against Desktops in **`ACTIVE` and `DEGRADED`.** Customer code errors (4xx) are excluded. **DEGRADED stays in the denominator** — a functionally limited state *is* reduced availability, and excluding it would erase the up-to-30-day DEGRADED window §19.4 permits |
| Desktop runtime availability (Windows) | **99.0%** | Procured from Azure (§19.3). Not our own infrastructure, so it depends on an upstream provider's SLA and is promised lower accordingly |
| Streaming connection success rate | 99% | From viewer token issuance to first frame within 30 seconds |

**Excluded from the calculation:** announced maintenance (up to 4 hours a month, 7 days' notice), customer-side network failures, and failures the customer caused inside the guest (§9.1 makes the guest interior the customer's domain).

**Credit policy**

Credit bands are defined by **shortfall against each service's own target**, not by absolute numbers. Since the targets differ (Linux 99.5%, Windows 99.0%), absolute bands would award a credit to a Windows Desktop that actually met its target.

| Shortfall against target | Credit | Linux (target 99.5%) | Windows (target 99.0%) |
|---|---|---|---|
| 0 – 0.5pp | 10% | 99.0% – 99.5% | 98.5% – 99.0% |
| 0.5 – 4.5pp | 25% | 95.0% – 99.0% | 94.5% – 98.5% |
| Over 4.5pp | 50% | < 95.0% | < 94.5% |

Credits are customer-requested and deducted from the following month. They apply **to compute charges only**, excluding pass-through items such as Windows licensing. Linux and Windows Desktops are **calculated separately**, so an outage in one does not generate credits for the other.

### 12.2 Incident runbooks

Runbook ids use `RB-n`, distinct from the risk register's `R-nn` (§17.1). They are separate schemes.

#### RB-1. Guest hang — the screen freezes and input does nothing

```text
Symptoms: repeated ACTION_TIMEOUT, streaming frames stopped
1. Check the iapetusd health probe (host → guest)
   ├─ daemon responds → X server / compositor problem.
   │                     Restart the display session via system.service
   └─ daemon silent   → guest OS level hang
2. Guest OS hang → force a restart at the hypervisor level
   - the volume survives; running applications are lost
3. Still failing after restart → transition to ERROR + notify the customer
                                 (webhook desktop.error)
Target recovery: 5 minutes
```

#### RB-2. Snapshot restore failure — resume does not work

```text
Symptoms: SUSPENDED → ACTIVE transition fails
1. If the memory snapshot is judged corrupt, fall back immediately:
   discard the memory snapshot → cold boot from the volume alone
   → processes are lost but login sessions and files survive
2. State the degraded resume explicitly in the response
   { "status": "ACTIVE", "warning": "process_state_lost" }
3. Volume also corrupt → offer restore from the most recent backup
                         snapshot (at most 24 hours old)
Target recovery: 2 minutes (fallback boot)
```

**Design principle:** a failure to preserve processes must **never escalate into data loss.** The memory snapshot and the volume are stored independently, and the memory snapshot is treated as a cache that can be discarded at any time.

#### RB-3. Streaming drops

```text
1. Automatic WebRTC ICE renegotiation (3 attempts)
2. On failure → force TURN/TCP
3. On failure → enter WebSocket JPEG fallback + show a badge in the viewer
4. Still failing → state in the UI that the Desktop itself is fine
   ("Only the screen feed failed; the agent's work is still running")
```

The point is that a streaming failure **must not be mistaken for a Desktop failure.** They are independent components.

#### RB-4. Host node failure

```text
A host carrying ACTIVE Desktops dies
1. Volumes live on network storage and survive
2. Reattach the volume on another host and cold boot
3. Process state is lost → notify the customer with process_state_lost
Target: back up within 10 minutes
```

**Volumes therefore must live on network storage, not node-local disk.** That conflicts with §2.2's warm-start KPI (local NVMe < 5s), so the two are split: **memory snapshots on local NVMe, volumes on network storage.** Losing a snapshot is recoverable as long as the volume survives.

### 12.3 Recovering data from an ERROR state

Even when a Desktop is unrecoverable, **the volume is alive.** The following guarantee that the user does not lose data.

| Possible | Method |
|---|---|
| Browse and download files | `GET /v1/desktops/{id}/files` (Desktop scope, works in ERROR) |
| Export the whole volume | `POST /v1/desktops/{id}/export` |
| Move the volume to a new Desktop | `POST /v1/desktops` with `clone_volume_from: dsk_...` |

An ERROR Desktop is **not deleted automatically.** Storage charges continue, and a deletion notice is sent after 30 days.

### 12.4 Capacity and placement

Since §8.3 split `spec_tier` into four tiers, a calculation premised on a single spec no longer holds. **The placement policy is fixed first and density derived per tier.**

#### Placement policy — one tier per host pool

**A host carries exactly one `spec_tier`.** Mixed placement fragments the remaining memory and the remaining encoding reservation into different units, forcing the scheduler to use a different formula every time.

Snapshot affinity already requires **splitting host pools by CPU template** (§7.4), so adding a tier axis is not a new mechanism.

```text
host pool = (CPU template) × (spec_tier)
   e.g. tmpl-a/standard, tmpl-a/large, tmpl-b/standard, …
```

- A resume is placed only in **the pool holding its snapshot.** The tier is fixed at creation, so the pool never changes.
- Changing tier is not supported. If needed, create a new Desktop and clone the volume (`clone_volume_from`).

#### Density per tier (32 vCPU / 128GB host, 16GB deducted for host overhead)

| `spec_tier` | vCPU/memory | ACTIVE per host | Tenant vCPU total | Tenant overcommit | Encoding reservation | Concurrent observation |
|---|---|---|---|---|---|---|
| `light` | 2 / 2GB | **56** | 112 | 3.50 : 1 ⚠ | 11 vCPU | 6 |
| `standard` | 2 / 4GB | **28** | 56 | 1.75 : 1 | 11 vCPU | 6 |
| `large` | 4 / 8GB | **14** | 56 | 1.75 : 1 | 11 vCPU | 6 |
| `gpu` | 4 / 8GB + GPU | **14** | 56 | 1.75 : 1 | Bound by GPU session limits | To be measured |

**The 2.5:1 ceiling is against total host physical vCPU, not tenant allocation.** The encoding reservation consumes real physical CPU, so excluding it would leave the ceiling governing nothing.

```text
(tenant vCPU total + encoding reservation 11) ÷ 32 physical ≤ 2.5
```

**`light` hits this ceiling.** Memory would fit 56, but CPU blocks first.

```text
available tenant vCPU = 2.5 × 32 − 11 = 69
light cap = ⌊69 ÷ 2⌋ = 34
check: (34×2 + 11) ÷ 32 = 2.47 ≤ 2.5 ✓   memory 34×2GB = 68GB ≤ 112GB (not binding)
```

| Final placement cap | Value | Binding constraint | Total overcommit |
|---|---|---|---|
| `light` | **34** | **CPU** (2.5:1 ceiling) | 2.47 : 1 |
| `standard` | **28** | Memory | 2.09 : 1 |
| `large` / `gpu` | **14** | Memory | 2.09 : 1 |

Given that light-tier workloads actually use CPU more often (browser rendering), 34 is not even conservative. **To be measured in Phase 1.**

#### Invariants

| Rule | Value | Rationale |
|---|---|---|
| Memory overcommit | **1 : 1 (forbidden)** | Swapping collapses GUI responsiveness. Applies regardless of tier |
| CPU overcommit ceiling | 2.5 : 1 | Measured as **(tenant vCPU + encoding reservation) ÷ physical vCPU.** Tiers that exceed it reduce their count |
| Encoding reservation | 11 vCPU per host, separate from tenant allocation | 6 × 1.8 vCPU = 10.8 (OpenH264, §6.3) |
| Concurrent observation | 6 per host | Derived from the reservation. Excess returns `NO_STREAM_CAPACITY` |
| SUSPENDED | Consumes no compute | But snapshot storage and host affinity still apply (§7.4) |

**The 11 vCPU encoding reservation is fixed regardless of tier,** because observation cost is set by screen resolution and has nothing to do with Desktop tier. As a result **the share each Desktop bears differs by tier.**

| Tier | Per host | Reservation vCPU per Desktop |
|---|---|---|
| `light` | 34 | 0.32 |
| `standard` | 28 | 0.39 |
| `large` / `gpu` | 14 | **0.79** |

The fewer Desktops there are, the fewer share the fixed reservation and the higher the unit cost. **Running observation-heavy workloads on `large` gives the worst economics**, and the answer there is the `gpu` tier — encoding moves to the GPU and the reservation disappears entirely.

**Six concurrent observers is low.** Workloads with an observation ratio above 20% — training, audit, demos — cannot be served by software encoding, so **the GPU tier becomes a requirement rather than an option.** The same is true for latency-sensitive use (§6.3).

**Windows is calculated separately:** its baseline memory requirement is 4–8GB and idle CPU usage is higher, putting it around 12 per host at `large`. Since procurement is from Azure (§19.3) the host specification itself differs, and this is recalculated in Phase 3.

**The figures in this table are measurement targets.** Density, overcommit, and encoding cost are measured and fixed at the end of Phase 1 (§18); only the placement policy — per-tier pools — is fixed now. A scheduler needs the policy to exist; the constants can be swapped in later.

### 12.5 Lightweighting levers

The density in §12.4 reflects the current image. The source of the weight is that **one Desktop is an entire OS plus a display session**, structurally 10–20× heavier than a headless alternative (§4.3). Below is the room to reduce that while keeping the structure, **all of it pending measurement before adoption.**

| # | Lever | Estimated saving | Risk |
|---|---|---|---|
| 1 | **Shared read-only rootfs** | 4–5GB per host | Low |
| 2 | **Lazy desktop environment** | 250–350MB per Desktop | Low |
| 3 | Chrome process-model tuning | 20–30% of Chrome RSS | Low |
| 4 | Free page reporting (balloon) | 15–30% of allocation | Medium |
| 5 | Aggressive `auto_suspend` tuning | Reduces the active count itself | Low |
| 6 | zram | Unknown | High |

**1. Shared read-only rootfs — the largest.** Today each Desktop has its own volume, so the Chrome binary, fonts, and libraries are **duplicated in the page cache once per Desktop.** At 28 Desktops the same Chrome is resident 28 times. Making the rootfs one read-only block device per host, with writes going to a per-Desktop overlay, removes the duplication.

**The overlay must be part of the persistent volume.** `app.install` writes to `/usr` and `/opt`, while §19.2 mounts only `/home/iapetus` and application data as persistent. Leaving the overlay volatile means **installed applications vanish on restart**, breaking the promise in §1.3 and §4.2 that installed applications carry into the next run. The ambiguity predates this lever, but the lever exposes it, so it is settled here.

**Side effect — one more axis on the host pool key.** A shared rootfs pins an image version to a host, so §12.4's pool key becomes `(CPU template) × (spec_tier) × (image version)`. It is the same constraint §19.2 already notes — that a memory snapshot pins its rootfs — raised to host granularity. **This is the same pattern as §19.4 injecting `iapetusd` as read-only squashfs**, and Firecracker can attach several block devices.

**2. Lazy desktop environment.** *(Only X11 window managers are candidates — capture (XGetImage), input (XTEST), and clipboard (X11 selection) are all X11 (§6.2), so switching to a Wayland compositor breaks all three at once.)* The XFCE session (xfwm4, xfsettingsd, thunar, panel, dbus) holds 300–400MB permanently, and **the agent barely uses it** — it launches applications directly and operates by coordinates, so a window manager is enough. In agent-only state, run just Xvfb plus a minimal X11 WM (openbox/i3, 10–30MB) and **start the panel and file manager when a human attaches to the viewer.** It is exactly the reasoning behind §6.3's "no viewers, no encoder": switch off the parts meant for people when no person is present.

**3. Chrome tuning.** The real top consumer is Chrome, not the desktop environment (800MB–1.5GB at five tabs). `--process-per-site` instead of site-per-process, `--disable-dev-shm-usage`, and disabled GPU compositing. Applied together with the `light` tier's tab ceiling (§8.3).

**4. Balloon reclamation is not counted in density.** A guest returning genuinely free pages to the host is not overcommit and does not violate §12.4's 1:1 rule. But **using the reclaimed memory for placement makes it overcommit at that moment.** It stays a buffer, and the density constants remain based on allocation. **It also interacts with snapshots** — a ballooned guest that is snapshotted and restored must have the balloon re-driven afterwards (§7.4 is strict about restore conditions, and skipping this double-counts memory).

**5. `auto_suspend` tuning may beat density.** As §12.4's break-even analysis shows, cost is governed not by Desktops per host but by **activity ratio.** Lowering `light`'s `idle_timeout_sec` from 900 to 120 directly reduces the active count. This is **a schema change making the default per-tier** (§8.3 currently has a single 900 default across tiers), and adopting it requires adding a per-`spec_tier` default table to §8.3. With warm start under five seconds (§2.2) the felt loss is small.

**6. zram is conditional.** §12.4 forbids swap on the basis of **disk** swap. zram is compressed RAM with microsecond latency and is a different thing. It could be considered at up to 25% of RAM on the `light` tier only, but **it consumes CPU under pressure**, so it is not adopted before measurement.

**Projection if adopted**

| | Today | After levers 1–3 (estimated) |
|---|---|---|
| `standard` effective memory | 4GB | 2.5–3GB |
| `standard` per host | 28 | **37–44** |
| `light` | 34 (CPU-bound) | **34 — unchanged** |

**`light` is already CPU-bound, so reducing memory does not raise its count.** On that tier only lever 3 (Chrome tuning) has effect. Failing to distinguish which lever applies to which tier wastes the work.

**What we will not do — a separate headless path for web-only work.** It would be the lightest option, but it creates a second runtime, and that market is one §4.3 already concedes we lose structurally. We **keep one runtime and switch off the heavy parts**, as in lever 2.

Every figure in this section is an estimate and is included in §18's measurement list.

### 12.6 Observability

| Layer | Metrics |
|---|---|
| Control Plane | Request rate, error rate, duration (RED); token issuance failure rate |
| Desktop | CPU, memory, disk IO; action latency histogram; capture FPS |
| Streaming | Connection success rate, bitrate, packet loss, fallback entry rate |
| Business | Desktop creation and deletion, ACTIVE hours, observation ratio, quota-hit rate |

**Action tracing:** a single `act` call is stitched into OpenTelemetry spans across API → Control Plane → `iapetusd` → X11. Without being able to answer "why was this click slow," the latency KPI cannot be improved.

**Fallback entry rate is a product metric.** A high value means the product experience is collapsing inside some customer's firewall, so it is treated as a customer-success metric rather than an infrastructure one.

---

## 13. Pricing Model

| Item | Unit |
|---|---|
| Compute | Desktop runtime seconds × tier |
| Storage | Volume GB-month |
| Snapshots | GB-month |
| Network | Outbound GB |
| Windows licensing | Additional per Desktop-hour (passed through at cost, zero margin) |
| Recording retention | GB-month |

`SUSPENDED` incurs no compute charge, which pushes agents to suspend often.

**Pricing principles**

1. **Reward suspending.** `auto_suspend` is on by default. Because an idle Desktop costs nothing, customers keep Desktops rather than deleting them — persistence is the product's value, so a price that encourages deletion is self-harm.
2. **Ordinary observation is not billed separately.** Charging a person for looking at their screen collides with §14's onboarding principle of showing them first. The encoding reservation cost (§12.4) is absorbed into the average compute rate. **There are two exceptions** — ① accounts with a persistently high observation ratio are steered to the GPU tier, and ② the forced-recording tier requires permanent encoding and carries a separate rate (§10.6).
3. **A quota overrun does not kill work.** Only new creation is blocked (§8.4).

**No prices until cost is verified:** pricing is set after Phase 1 produces measured per-Desktop cost (compute, storage, encoding, network). This mitigates R-02 (unit economics); until then this table is **a list of billable items, not a rate card.**

---
## 14. Onboarding and User Journey

### 14.1 Agent developer: signup to first action

**Goal: first success within 15 minutes.** Past that, they leave.

```text
1. Sign up → project created automatically → Project Key issued        (1 min)
2. Click "Create Desktop" in the dashboard, or one API call            (15s + 15s boot)
3. The Desktop's screen appears in the dashboard immediately  ← the decisive moment
4. Install the SDK and run the five-line example                       (3 min)
5. Watch that operation move live in the browser              ← "oh, this actually works"
```

Steps 3 and 5 are the whole of onboarding. **Without seeing the screen, this product is not understood.** A developer who runs code and cannot tell what happened leaves. So creating a Desktop from the dashboard **must open the viewer by default.**

**The first example must produce something visible.**

```python
from iapetus import Iapetus

client = Iapetus(api_key="sk_iap_...")
desktop = client.desktops.create(
    name="my-first-desktop",
    owners=[{"type": "agent", "id": "agent_me"}],   # a human is added later under the bootstrap exemption (§8.1)
)

print(f"👀 Watch it here: {desktop.viewer_url}")   # ← printed first

with desktop.session() as c:
    c.launch_app("chrome", wait_for_window=True)
    c.type("iapetus")
    c.key("Enter")
```

Printing `viewer_url` **before the work starts** is deliberate: it gives the user time to open the browser before anything moves.

### 14.2 Human Owner: first-time setup (S5)

The recommended path that avoids handing credentials to the agent.

```text
[customer product UI]
   "Connect messenger" button
         │
         ▼
customer server issues a Viewer Token (control: write = maximum requestable)
         │
         ▼
[Iapetus viewer opens — embedded or a new window]
   ┌──────────────────────────────────────┐
   │  🖥  My Desktop                       │
   │  ┌────────────────────────────────┐  │
   │  │                                │  │
   │  │   (the actual desktop screen)  │  │
   │  │                                │  │
   │  └────────────────────────────────┘  │
   │  [Open messenger]  ← guidance button │
   │  Status: observing  [Take control]   │
   └──────────────────────────────────────┘
         │
         ▼
user clicks [Take control] → control/acquire → WRITE granted (§8.1)
         │
         ▼
user signs in personally, 2FA included — neither we nor the agent see the credentials
         │
         ▼
   "Connected" → close the viewer → the agent takes over
```

**Guidance overlay:** the viewer can display step-by-step instructions supplied by the customer, for example "click the messenger icon." A user encountering a remote desktop for the first time does not know what to do.

### 14.3 Steady-state journey

| Situation | What the user does |
|---|---|
| Normally | Nothing. The agent handles it |
| Wanting to check | Glance at thumbnails on the dashboard (§6.3), click through to live observation if curious |
| The agent is stuck | Receive a notification → open the viewer → preempt → resolve → hand back |
| Reviewing results | Download the file, or open it directly on the Desktop |
| Investigating a problem | Replay the session recording and audit log |

### 14.4 Local development

Before touching the SDK, a developer must be able to **run it locally without paying.**

```bash
# one line to run Iapetus locally (Control Plane + one Desktop + viewer)
docker compose up

# → API:     http://localhost:8080
# → Viewer:  http://localhost:3000
# → Desktop: created automatically (dsk_local)
```

| Works locally | Does not |
|---|---|
| The whole Computer API | Multi-tenant isolation (runc, §19.2) |
| Viewer and streaming | Process-preserving suspend/resume |
| Linux Desktops | Windows Desktops |
| Persistent volumes | Multi-region, warm pools |

**Changing only the endpoint makes the SDK behave identically against local and cloud.** There is no local-only code path.

### 14.5 Onboarding KPIs

| Metric | Target |
|---|---|
| Signup → first Desktop created | Median under 3 minutes |
| **Signup → first successful action (time to first action)** | **Median under 15 minutes** |
| Opened the viewer during the first session | > 80% (we assume churn spikes among those who do not; to be validated) |
| First action succeeded without reading documentation | > 50% |
| 7-day retention (returned after signup) | > 40% |

**When measurement begins:** these metrics are meaningful only **from Phase 2**, when self-service signup opens. Phase 1 has no multi-tenant isolation (§19.2) and runs as an invite-based pilot, where qualitative interviews substitute.

---

## 15. Test and Verification Strategy

A GUI automation platform fails differently from an ordinary backend. **The screen is non-deterministic, the guest OS is a black box, and latency is itself a feature.** The test strategy is shaped around that.

### 15.1 Test layers

| Layer | Subject | Tooling | When |
|---|---|---|---|
| **L1 unit** | `iapetus-proto` serialization, coordinate conversion, policy evaluation, token verification | `cargo test` | Every commit |
| **L2 guest integration** | Whether input and capture work inside a real Xvfb | In-container integration tests | Every commit |
| **L3 API contract** | REST/WS schemas, error codes, idempotency, authentication | OpenAPI-driven contract tests | Every commit |
| **L4 E2E scenarios** | The full S1–S6 flows | Real Desktops with real applications | Every merge + nightly |
| **L5 load and endurance** | Concurrent Desktop count, streaming density, seven-day continuous operation | k6 plus a bespoke harness | Weekly |
| **L6 chaos** | Forced host termination, snapshot corruption, network partition | Fault injection | Before release |

### 15.2 L2 guest integration — the critical tests

This layer matters most. A bug missed here surfaces only in production.

```rust
#[test]
fn click_coordinates_match_the_actual_window() {
    let d = TestDesktop::launch();           // Xvfb + a test application
    d.open_target_window(400, 300, 100, 50); // a button at a known position
    d.input().click(425, 315, Button::Left, 1).unwrap();
    assert_eq!(d.target_app().last_click(), Some((25, 15))); // window-relative
}

#[test]
fn hangul_input_arrives_intact_without_recomposition() {
    let d = TestDesktop::launch();
    d.focus_text_field();
    d.input().type_text("안녕하세요", Duration::from_millis(10)).unwrap();
    assert_eq!(d.text_field_value(), "안녕하세요");  // no jamo splitting
}

#[test]
fn a_scaled_screenshot_still_reports_physical_pixel_coordinates() {
    let d = TestDesktop::launch();
    let shot = d.display().screenshot_scaled(0.5).unwrap();
    assert_eq!(shot.image_size(), (960, 540));
    assert_eq!(shot.display.width, 1920);   // the coordinate frame is unchanged (§7.2)
}
```

**The Hangul input test is mandatory in CI.** IME problems are fatal in the Korean market and are exactly the area anglophone open-source stacks leave unverified.

### 15.3 Handling screen non-determinism

Pixel comparison of screenshots is **not used.** Font hinting, antialiasing, cursor blink, and animation make every capture different.

| Problem | Response |
|---|---|
| Minute pixel differences | No pixel comparison. Verify by **semantic assertion** — window title, application state, file contents |
| Capturing mid-animation | `wait_for(mode: screen_stable)` — wait until two consecutive frames differ below a threshold |
| Font rendering differences across environments | Pin font versions in the test image; reproducibility comes from the image tag |
| Variation in application start time | No fixed sleeps. `wait_for_window` with a timeout |
| Screens that depend on the network | E2E uses **a local test page** rather than an external site |

Where screenshot regression is genuinely required (viewer UI and similar), use perceptual diff at an SSIM threshold of 0.98 with a human approval step on failure.

### 15.4 Scenario-to-acceptance-test mapping

The §3.2 scenarios become automated tests directly. **The PRD's scenarios and the tests correspond one-to-one**, which exposes any missing requirement.

| Scenario | Acceptance test | What it verifies | Phase |
|---|---|---|---|
| S1 web search | `e2e/s1_chrome_search` | Launch → input → reaching the result screen | 1 |
| S2 messenger send | `e2e/s2_messenger_send` | Reuse of the login state, message actually delivered | 3 |
| S3 scheduled routine | `e2e/s3_scheduled_routine` | **Login still valid after seven consecutive days** | 2 |
| S4 human intervention | `e2e/s4_preempt` | Preemption within 500ms, agent receives `control.revoked` | 2 |
| S5 human first setup | `e2e/s5_human_setup` | Agent successfully reuses the session after a human signs in | 2 |
| S6 reviewing results | `e2e/s6_shared_file` | Both parties see and edit the same file | 2 |

**S3 is the most important test.** It is the only one that proves the product's central claim — persistence — and its seven-day axis cannot be substituted by anything else. It runs as **a permanently operating long-horizon canary**, separately from CI.

### 15.5 Load targets

| Scenario | Target |
|---|---|
| Concurrent ACTIVE Desktops | 10,000 per region (§11) |
| Concurrent streaming viewers | Frame latency held under 200ms across 1,000 sessions |
| Action throughput | p95 under 300ms at 20 req/s per Desktop |
| Creation burst | Cold-start KPI held at 500 creations per minute |
| Seven-day endurance | No memory leak; action latency growth under 10% |

### 15.6 Phase completion criteria (tied to tests)

A Phase is complete only when **its tests are green in CI.** "It seems to work" is not completion.

| Phase | Must pass |
|---|---|
| 1 | All of L1, L2, L3 plus `e2e/s1_chrome_search` |
| 2 | Plus `s3` (seven-day canary), `s4`, `s5`, `s6`, and L6 chaos (RB-1, RB-2) |
| 3 | Plus `s2` and all of Windows L2 |
| 4 | Plus every L5 load target |

---

## 16. Roadmap

### Phase 1 — MVP (Linux, Docker)
- Linux XFCE OCI image on the Docker runtime (self-host and development)
- The `iapetusd` Rust daemon and the shared `iapetus-proto` crate
- Desktop create / read / delete, persistent volumes
- Computer API: screenshot, click, type, key, scroll, launch_app, shell.exec
- OWNER authority model (root + sudo), arbitrary execution and installation
- Preinstalled catalog: chrome, terminal, files, text_editor
- REST API plus Python and TypeScript SDKs
- Human viewer: streaming, full input control, and an **exclusive control lease** (preemption and events come in Phase 2). Without a lease, a human and an agent typing simultaneously corrupt the screen (§5.6), so even Phase 1 needs minimal mutual exclusion
- **Completion criterion:** scenario S1 (Chrome search) succeeds through the API alone

### Phase 2 — Persistence, intervention, multi-tenant isolation
- **Move to Firecracker microVMs** (same image definition, OCI converted to rootfs; Kata not adopted for lack of snapshots, §6.4)
- Suspend/resume via memory snapshot, auto-suspend
- `window.*` API, `wait_for`, batched `act`
- Control lease arbitration (human preemption and release) plus the event stream
- Audit log, session recording
- **Policy engine, `secret.type`, and the egress proxy** — required before SaaS opens; multi-tenancy cannot launch without them
- MCP server
- **Completion criterion:** scenario S3 (scheduled routine with session reuse) succeeds seven days running

### Phase 3 — Windows
- Windows VM runtime, DXGI capture, SendInput
- Desktop application catalog: corporate messenger, Excel, and similar
- Organization custom image builds
- Windows viewer special keys (Ctrl+Alt+Del)
- **Completion criterion:** scenario S2 (messenger send) succeeds with the login preserved

### Phase 4 — Scale
- Multi-region, snapshot and restore, GPU Desktops for observation-heavy workloads
- Semantic accessibility-tree control, reducing coordinate dependence (mitigates R-05)
- Embedded viewer SDK, mobile viewer
- Corporate network connectivity (VPN/PrivateLink), audio (one-way, v2)
- SOC 2 Type II observation window begins (§10.5)
- **Completion criterion:** every §15.5 load target met

### Per-phase validation gates

A Phase ends on **hypothesis validation**, not on a feature list (§17.2).

| Phase | Under test | If it fails |
|---|---|---|
| 1 | Technical feasibility — can GUI operation be driven reliably through an API | If coordinates prove limiting, pull the accessibility tree forward into Phase 2 |
| 2 | **H-2 persistence** plus H-3 uptake of human intervention | H-2 failing removes the product's rationale → §17.3 kill criteria trigger |
| 3 | H-5 demand for desktop applications | Insufficient demand narrows scope to web and internal software |
| 4 | H-1 willingness to pay plus H-4 unit economics | Reprice, or pivot to enterprise-only |

---
## 17. Risk Register

### 17.1 Risks

Likelihood and impact are **high / medium / low** (tenant compromise alone is rated **critical**). Priority comes from the combination.

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R-01 | **Third-party application policy change** — a major desktop application blocks automation or virtualized execution | High | High | Do not make the product depend on any one application. An application is one catalog entry; if blocked, only that entry is removed. Keep a web-version fallback. Hold to the own-account principle |
| R-02 | **Unit economics collapse** — the cost of keeping a Desktop exceeds willingness to pay | High | High | `auto_suspend` on by default, a light 2GB tier, a GPU tier separated by observation ratio. Price only after Phase 1 produces measured cost |
| R-03 | **Tenant compromise via container escape** | Medium | **Critical** | microVM kernel isolation is mandatory (§19.2). Forbidding shared-kernel multi-tenancy is enforced as a release gate |
| R-04 | **Abuse of full authority** — mining, spam, scanning | High | Medium | **What is actually enforced on the standard tier is malicious-destination blocking, private-range blocking, and anomaly detection — not a domain allowlist** (§9.1). The primary defences are free-tier resource caps, payment verification, and mining signature detection |
| R-05 | Low reliability of GUI automation — coordinates are fragile against UI change | High | Medium | Accessibility-tree hybrid in v2. Document `wait_for` patterns. Clear errors so the agent can recognize failure |
| R-06 | Windows cost and licensing barrier | Medium | High | Defer to Phase 3 and validate the market on Linux. Procurement is fixed to **Azure** (§19.3), removing SPLA negotiation lead time; the residual risk is cost (the Azure premium) and the burden of running two regions |
| R-07 | **A customer's agent misuses end-user accounts** | Medium | High | Cannot be prevented technically. Terms, DPA, and notice obligations; the Desktop-as-trust-unit guidance (§9.3); audit logs provided |
| R-08 | Corporate firewalls blocking WebRTC collapse the viewer experience | Medium | Medium | TURN over TCP/443 plus JPEG fallback (§6.3). Track fallback entry rate as a customer-success metric |
| R-09 | Enterprise sales fail for lack of certification (ISMS-P and similar) | Medium | Medium | Accumulate evidence from Phase 1 (§10.5). The self-host option provides a way around |
| R-10 | A large platform (cloud or model provider) ships a similar capability | Medium | High | Differentiate on desktop applications, the Korean work environment, and the co-ownership model. Avoid competing on generic code sandboxes (§4.3) |
| R-11 | Schedule slip from Rust development speed, particularly on Windows | Medium | Medium | Hold Rust for `iapetusd` only and stay flexible elsewhere. Windows is isolated in Phase 3 |
| R-12 | Snapshot and persistence reliability below target (lost logins) | Medium | High | Separate volume from memory snapshot (§12.2 RB-2). Design login sessions to depend on the volume alone |

**Top three:** R-03 (isolation), R-02 (economics), R-01 (application policy) — handled respectively as a release gate, Phase 1 measurement, and management of product dependence.

### 17.2 MVP hypotheses

Phases 1 and 2 are treated as **hypothesis validation**, not feature delivery.

| # | Hypothesis | How it is tested | Failure threshold |
|---|---|---|---|
| H-1 | People pay for a "persistent desktop" | Paid conversion measured at the end of Phase 2 | Free-to-paid conversion < 5% |
| H-2 | Login session persistence actually holds | Run the S3 canary as **50 in parallel × 30 days ≈ 1,500 resume cycles** | **Two or more** losses (≈99.87%). A single canary gives only 30 cycles, which cannot statistically verify a 99.9% KPI |
| H-3 | Users actually use the human intervention model | Viewer connection rate and preemption frequency | First-week viewer connection rate < 30% |
| H-4 | Unit economics work | Measured cost per Desktop versus price | Margin < 30% |
| H-5 | Demand for desktop application automation exceeds web | Distribution of application usage after Phase 3 | Desktop applications under 20% of total usage |

### 17.3 Kill criteria

Without setting these in advance, sunk cost keeps the project going.

| Condition | Decision point | Action |
|---|---|---|
| H-2 fails (persistence not trustworthy) | End of Phase 2 | **The product's rationale is gone.** Stop, or pivot to single-use sandboxes |
| H-4 fails with no path to improvement | End of Phase 2 | Reprice, or narrow to enterprise self-host only |
| R-03 materializes (tenant compromise) | Immediately | Stop multi-tenant SaaS; convert to single-tenant only |
| R-01 materializes broadly (several major applications blocked) | Ongoing | Drop the desktop application axis and narrow to web plus internal software |
| Fewer than 10 paying customers at the end of Phase 3 | End of Phase 3 | Judge the market absent; narrow or stop |

**Pivot options:** stopping is not the only choice. ① An on-premise internal automation product, ② test infrastructure for agent developers, ③ a VDI-plus-AI product. Maintaining the self-host path (§19.2) from the start is what keeps these open.

---

## 18. Open Issues

**Every contested design decision is closed.** All of them have moved into the body as settled design; the strikethroughs below preserve the history.

**But "decided" must not be read as "ready to implement."** This table only ever covered *what people disagreed about.* The work nobody disputes — the plain enumeration of fields, types, caps, envelopes, and conventions — sat empty precisely because it was uncontested, and was filled in v0.7 as §8.2, §8.3, §19.5, and §19.6. The remaining interface work is listed below.

**What remains lies outside this document.** These are drafting, external review, measurement, and reference writing rather than design, and they are not completion conditions for this specification.

| Follow-up | Nature | Needed by |
|---|---|---|
| Terms of service drafting (own-account only, no bulk activity) | Legal | Before Phase 2 launch |
| DPA and end-user notice templates | Legal | Before Phase 2 launch |
| Azure Windows cost measurement and contract | Procurement | Before Phase 3 begins |
| Measured cost per Desktop (R-02, H-4) | Measurement | End of Phase 1 |
| OpenH264 encoding cost measurement (validating §6.3's 1.2–1.8 vCPU) | Measurement | End of Phase 1 |
| Lightweighting lever measurement (the six in §12.5) | Measurement | End of Phase 1 |
| **API reference authoring** — full request and response per endpoint | Documentation | Before the Phase 1 API sprint |
| `iapetus-proto` crate — Protobuf types and OpenAPI generation | Implementation | At Phase 1 start |

**How to tell what belongs here versus in the API reference:** one test decides it — **does the field's existence define a capability, or merely serialize a capability already decided?**

- If its existence defines a capability, **the PRD owns it.** For example Image's `source` (it decides whether we operate a build farm), `PUT .../policy` (it decides whether per-Desktop policy exists in v1), and `viewer_url` (it is the mechanism by which onboarding's decisive moment works).
- If the capability is settled and only its shape remains, **the reference and `iapetus-proto` own it.** For example that `window.list` returns id/title/bounds/focused, or the body of `GET /v1/health`.

The rule exists so a reference author has a way to decide "does this need to go back to the PRD?"

The PRD carries only **decisions an implementer must not make alone** — the identifier scheme, pagination, the error envelope, caps, the idempotency contract, and the transport protocol (§8.2, §8.3, §19.5). Per-endpoint field lists belong to the reference and `iapetus-proto`. The boundary exists because writing the same content in two documents guarantees they diverge.

| # | Issue | Options | Note |
|---|---|---|---|
| 1 | ~~Windows license procurement~~ | — | **Decided.** §19.3: Phase 3 procures from Azure (license included); revisit SPLA above 500 Desktops a month. Region bifurcation accepted as the cost |
| 2 | ~~Terms-of-service risk in third-party app automation~~ | — | **Decided.** The three §7.3 principles: we do not distribute applications / the user's own account only / no dependence on any application |
| 3 | ~~Coordinates versus accessibility tree~~ | — | **Resolved.** v1 is coordinate-based (§7.2 coordinate convention); the accessibility-tree hybrid is Phase 4 (§16). Residual risk tracked as R-05 |
| 4 | ~~Control lease model~~ | — | **Resolved.** §5.6: exclusive lease with immediate human preemption |
| 5 | ~~Where screenshots are stored~~ | — | **Resolved.** §8.2 responses return a CDN presigned URL; retention in §10.2 |
| 6 | ~~Desktop ↔ agent ownership relation~~ | — | **Resolved.** §5.2's multi-Owner model (humans and agents, N:M) |
| 7 | ~~Reducing cold start~~ | — | **Resolved.** Warm pools differentiated by tier. The KPI is stated without warm pools in §2.2 |
| 8 | ~~v1 isolation backend~~ | — | **Resolved.** §19.2: Docker for self-host, Firecracker microVM for SaaS. A release gate for R-03 |
| 9 | ~~Abuse of full authority~~ | — | **Resolved (design).** Host-level control per tier (§9.1) plus §9.5. Residual risk is tracked as R-04 and not duplicated here |
| 10 | ~~Agent developer versus end-user trust~~ | — | **Decided.** §9.3: not prevented technically but answered with transparency and control (Owner visibility, right to evict, audit access, always-on observation) plus DPA notice obligations |
| 11 | ~~Multiple human Owners~~ | — | **Decided.** §5.2: `desktop_type` split into `personal` (one human) and `shared` (many, personal accounts discouraged with a persistent banner). Conversion is irreversible and requires explicit consent |
| 12 | ~~`iapetusd` update path~~ | — | **Resolved.** §19.4: read-only squashfs block device injection plus protocol version negotiation (N-2 compatible) |
| 13 | ~~Egress proxy circumvention~~ | — | **Resolved.** §9.1: enforced at the host vNIC. Standard tier uses a denylist plus SNI; the hardened tier adds DNS DNAT and DNS-to-IP correlation. MITM decryption is not adopted |
| 14 | ~~Concurrent requests to one Desktop~~ | — | **Resolved.** §5.6: no queuing, immediate `CONTROL_HELD` with `retry_after_sec`. Concurrency is solved by splitting Desktops |

---

## 19. Technology Stack

### 19.1 Language: Rust

| Component | Language | Rationale |
|---|---|---|
| **iapetusd** (in-guest daemon) | **Rust** | A single static binary drops into any image with no runtime dependency. Safe X11/Win32 FFI. It runs permanently, so low latency and low memory without GC pauses pay off directly |
| **Control Plane** (API and orchestrator) | **Rust** (axum + tokio) | Holds thousands of WebSocket connections. Mature async runtime. Shares the protocol type crate with iapetusd |
| **Streaming gateway** | **Rust** (webrtc-rs) | Minimizes GC and copying on the frame path |
| SDKs | Python / TypeScript | The consumers are agent developers; this side is not Rust |
| CLI | Rust (clap) | Reuses the Control Plane crates |
| Dashboard | TypeScript / React | |

**The real reasons for Rust (not only performance)**
- `iapetusd` ships inside customer images. A Python runtime or JVM cannot ride along. A statically linked single binary is effectively a requirement.
- X11 (XTEST), DXGI, and SendInput are all unsafe FFI. Rust confines that unsafety to narrow modules and keeps the rest safe.
- Action protocol types live in one `iapetus-proto` crate shared by the Control Plane and iapetusd, eliminating schema drift. JSON Schema and OpenAPI for the SDKs are generated from it.

**The cost of Rust (acknowledged and accepted)**
- Initial development is slower than Go or TypeScript. The Windows capture and input layers especially so.
- Windows support carries the learning cost of the `windows-rs` crate.
- Mitigation: prototype v0 in Python if useful, but **write `iapetusd` in Rust from the start** — it is the component hardest to change later.

**Proposed workspace layout**
```text
iapetus/
├── crates/
│   ├── iapetus-proto/      # action and event schemas (shared)
│   ├── iapetusd/           # guest daemon
│   │   ├── input/          # linux(xtest) | windows(sendinput)
│   │   ├── capture/        # linux(x11/pipewire) | windows(dxgi)
│   │   ├── apps/           # launch / install / process
│   │   └── fs/
│   ├── iapetus-control/    # Control Plane (axum)
│   ├── iapetus-stream/     # WebRTC gateway
│   └── iapetus-cli/
├── sdk/
│   ├── python/
│   └── typescript/
└── images/                 # Dockerfile / image builds
```

Platform differences are abstracted behind traits.
```rust
pub trait Display: Send + Sync {
    fn screenshot(&self, region: Option<Rect>) -> Result<Frame>;
    fn size(&self) -> ScreenInfo;
}
pub trait Input: Send + Sync {
    fn click(&self, x: i32, y: i32, button: Button, count: u8) -> Result<()>;
    fn type_text(&self, text: &str, delay: Duration) -> Result<()>;  // includes IME
    fn key(&self, combo: &KeyCombo) -> Result<()>;
}
// #[cfg(target_os = "linux")] → X11Display / XTestInput
// #[cfg(target_os = "windows")] → DxgiDisplay / SendInputDriver
```

### 19.2 Packaging: Docker — but not as an isolation boundary

Docker is used as **an image build and distribution format.** That decision is sound.

| Use | Docker's fit |
|---|---|
| Desktop image definition (Dockerfile), layer caching, registry distribution | ✅ Ideal |
| Local development / self-host / single-tenant deployment | ✅ Sufficient |
| **Isolating mutually unknown customers in multi-tenant SaaS** | ❌ **Unsuitable** |

**Why:** §7.3 grants the agent guest root. Root inside a Docker container shares the host kernel. One kernel vulnerability allows container escape and compromise of another customer's Desktop. **A container that grants root is not a security boundary.**

**Conclusion: Docker for images, microVM for execution.**

| Deployment | Runtime |
|---|---|
| Local development / self-host / single organization | Docker (`docker run`) |
| Multi-tenant SaaS | **Firecracker with OCI→rootfs conversion** (Kata not adopted for lack of snapshots, §6.4) |
| Windows Desktop | A dedicated VM, not a container |

**One image definition is maintained.** Self-host runs the OCI image directly; SaaS converts the same image to a rootfs for Firecracker. A customer building a custom image writes one Dockerfile.

```yaml
# self-host: runc (shared kernel, single tenant only)
# SaaS:      Firecracker microVM (OCI → rootfs conversion, snapshot support)
#            the image definition (Dockerfile) is identical for both
```

**Linux Desktop image (Dockerfile outline)**
```dockerfile
# Single image for development and self-host. Production uses the squashfs block
# device injection described in §19.4.
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --release -p iapetusd && \
    mkdir -p /out && cp target/release/iapetusd /out/

FROM debian:bookworm-slim

# fonts-noto-cjk / ibus-hangul: required for Hangul display and input
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb xfce4 xfce4-terminal dbus-x11 \
      fonts-noto-cjk ibus ibus-hangul \
      sudo ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# Chrome and the other default catalog applications
COPY scripts/install-apps.sh /tmp/
RUN /tmp/install-apps.sh

# The shared OS account — one human and agent both use (§5.2).
# NOPASSWD sudo; there is no separate password, hence no password_ref concept.
RUN useradd -m -s /bin/bash iapetus \
 && echo 'iapetus ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/iapetus

# The Rust daemon: one static binary
COPY --from=builder /out/iapetusd /usr/local/bin/iapetusd

ENV DISPLAY=:1
USER iapetus
ENTRYPOINT ["/usr/local/bin/iapetusd", "--supervise-x11"]
```

`iapetusd` supervises the Xvfb and XFCE session itself. No separate supervisord is added, because the daemon must know the process tree for `app.list` and `process.kill` to be accurate.

**A base image upgrade does not reach SUSPENDED Desktops immediately.** A memory snapshot binds the kernel and rootfs of its moment, so raising the image version still wakes the old image on resume. It applies at **the next cold boot** (a restart, or a resume after discarding the snapshot); where a security patch is required, customers are given advance notice of a restart (the same procedure as §19.3).

**Persistence and the image:** the image stays stateless, and `/home/iapetus` (the shared account's home) plus application data are mounted from **a persistent volume.** Replacing the image with a new version keeps Chrome cookies and messenger logins. Process preservation across suspend/resume is incomplete with containers (it depends on CRIU, which is fragile), so it is **fully guaranteed only in the SaaS environment with microVM snapshots.** On self-hosted Docker it is stated as `restart` level, volume only.

### 19.3 Windows procurement

**The problem:** reselling Windows as multi-tenant SaaS is not possible under an ordinary volume licence and requires a Microsoft SPLA agreement. SPLA brings minimum commitments, monthly reporting, and audit obligations — a heavy burden for an unvalidated product to take on early.

**Decision: Phase 3 procures Windows instances from Azure.**

| Approach | Adopted | Rationale |
|---|---|---|
| **Azure Windows instances** | ✅ Phase 3 | The licence is included in the instance price, so the cloud provider resolves resale eligibility. Zero contract lead time |
| Our own SPLA agreement | △ After Phase 4 | Above roughly 500 Windows Desktops a month the economics beat Azure. Revisit then |
| Ordinary volume licence only | ❌ | Cannot be used for multi-tenant resale. Legal risk |

**The cost is stated plainly:** Linux on our own infrastructure and Windows on Azure means **regions, networking, and operations bifurcate.** Windows Desktop latency and availability KPIs are measured separately against the Azure region, and §12.1's SLA is calculated separately too. We accept that complexity because it beats letting an SPLA negotiation consume the entire Phase 3 schedule.

### 19.4 `iapetusd` distribution and updates

**The problem:** baking the daemon into the image (§19.2's Dockerfile) pins a specific version into customer-built custom images (D-09) as well. Every protocol change or security patch would then **require the customer to rebuild the image**, and at that moment the platform loses the ability to update itself.

**Decision: side-load it from a volume.** The daemon is not baked into the image.

Firecracker has no virtio-fs (§6.4), so file-level injection is impossible. **The daemon is attached as a read-only squashfs block device.**

```text
Desktop boot
   │
   ├─ the host attaches a read-only squashfs **block device** → the guest mounts it at /opt/iapetus
   │
   └─ the image's ENTRYPOINT is a thin shell stub that execs /opt/iapetus/bin/iapetusd
```

| Approach | Adopted | Reason |
|---|---|---|
| Bake into the image | ❌ | Customer images pin the version |
| **Read-only volume injection** | ✅ | The host decides the version. A reboot is enough to update. Customer images stay untouched |
| Download at boot | ❌ | Adds cold-start latency and fails to boot during a network incident |
| Sidecar container | ❌ | Reaching the guest display and input requires the same namespace |

**The Dockerfile above (§19.2) is the single-image form used for development convenience.** The production image drops the `COPY --from=builder /out/iapetusd` line and its ENTRYPOINT becomes the stub.

**Version negotiation:** the Control Plane and `iapetusd` exchange protocol versions on connect.

```json
{ "daemon_version": "1.7.2", "protocol": { "min": 3, "max": 5 } }
```

- Where the ranges overlap, **the highest common version** is used.
- Where they do not, the Desktop is not marked `ERROR` but **`DEGRADED`**, and it recovers automatically on the next resume by attaching a new daemon volume.
- **A Desktop with `auto_suspend: false` never resumes on its own and can therefore sit DEGRADED indefinitely.** Such Desktops trigger a customer notification after 7 days and a maintenance-window restart after 30.
- Protocol changes remain **backward compatible to N-2**, so daemons never have to be replaced all at once.

**Zero-downtime updates are explicitly not possible:** replacing the daemon requires a reboot or a suspend/resume. Running Desktops are left alone, and **only for an urgent security patch** does the platform force a restart, with 24 hours' notice by webhook and dashboard.

### 19.5 `iapetusd` ↔ Control Plane transport contract

**Without this section two teams build two different protocols.** "The types are in the `iapetus-proto` crate" is a circular reference to code that does not yet exist, so the contract is fixed here.

#### Connection model

```text
guest (iapetusd)                          Control Plane
  │                                             │
  │──① the guest dials out (mTLS + Guest Token)─►│
  │   session affinity: one Desktop = one CP node │
  │                                             │
  │◄─② HELLO / protocol version negotiation ───►│
  │                                             │
  │◄─③ one long-lived bidirectional multiplexed stream ─►│
  │   CP→guest: action requests | guest→CP: responses, events │
  │                                             │
  │──④ heartbeat every 5s ─────────────────────►│
```

**Why "outbound only" and "delivering actions" are not contradictory:** the guest dials the connection, but the stream is bidirectional. The Control Plane merely issues actions **over that connection**; it never connects to the guest. The guest still opens no inbound port and holds no authority to call Control Plane APIs (§9.1).

**RB-1's host→guest health probe is a separate path.** Distinguishing "the daemon died" from "the guest OS died" when the stream is gone requires a path independent of the network. The mechanism differs per runtime.

| Runtime | Health probe path | Phase |
|---|---|---|
| Docker (Phase 1, self-host) | Container PID 1 state plus a `docker exec` probe | Phase 1 |
| Firecracker (SaaS) | **vsock** — a hypervisor-internal channel | Phase 2 |
| Windows / Hyper-V | hvsocket | Phase 3 |

None of the three traverses the guest's network namespace, so §9.1's inbound-blocking principle holds. **Assuming vsock while Phase 1 runs on Docker would make RB-1's first step impossible**, which is why the per-runtime path is fixed here.

#### Transport and framing

| Item | Choice | Rationale |
|---|---|---|
| Transport | **gRPC over mTLS** (HTTP/2 bidirectional streaming) | Multiplexing, flow control, and reconnection come built in. Mature Rust ecosystem |
| Serialization | **Protobuf** (`iapetus-proto`) | The Control Plane and the guest share types, eliminating schema drift (§19.1) |
| Stream | One long-lived bidirectional stream per Desktop | Actions, events, and heartbeats multiplexed on one channel |

```protobuf
message Frame {
  uint64 id = 1;              // request/response correlation, unique within the connection
  oneof body {
    ActionRequest  request   = 2;   // CP → guest
    ActionResponse response  = 3;   // guest → CP
    Event          event     = 4;   // guest → CP
    Heartbeat      heartbeat = 5;   // both directions
    StreamChunk    chunk     = 6;   // shell.stream, file transfer
  }
}
```

- **Requests on one connection execute in arrival order.** The guest never reorders actions, and this is what backs §7.2's sequential `act` execution and §8.5's WebSocket FIFO guarantee.
- In-flight depth is capped at **8.** Excess is naturally held back by HTTP/2 flow control.
- `ActionRequest` carries `deadline_ms`. **Past it the guest drops the response rather than sending one.** Input already applied is not undone (§8.2).

#### Reconnection and in-flight handling

```text
connection lost
  → the guest reconnects with exponential backoff (1s → max 30s, jittered)
  → in-flight actions are NOT retransmitted
  → the Control Plane answers those requests with ACTION_TIMEOUT
  → the control lease survives (until three heartbeats are missed)
```

**Not retransmitting is the point.** Resending an action whose execution during the outage is unknown puts the click through twice. The retry decision belongs to the agent, which holds the idempotency key (§8.4).

Three consecutive missed heartbeats (15 seconds) mark the Desktop `DEGRADED`; sixty seconds triggers RB-1 (§12.2).

#### Version negotiation

```json
{ "daemon_version": "1.7.2", "protocol": { "min": 3, "max": 5 } }
```

- Use the highest value in the overlapping range. With no overlap, mark `DEGRADED` and recover with a new daemon volume on the next resume (§19.4).
- Maintain **N-2 backward compatibility** so daemons need not be replaced in lockstep.
- This integer version is **internal and never exposed to API clients** (§8.2).

#### What the guest is responsible for, and what is not trusted

| Guest responsibility | Reference |
|---|---|
| Supervising the display session (no separate supervisor) | §19.2 |
| Maintaining the Frame Source and **masking before encoding** | §10.6 |
| Stopping the video encoder when no viewers remain | §6.3 |
| Forcing a clock resync immediately after resume, holding actions until it completes | §7.4 |
| Force-releasing held keys and buttons on lease handover | §5.6 |

Both humans and agents are guest root, so the daemon is assumed to be tamperable (§9.1). The following therefore **never rely on values the guest reports.**

| Item | Actual basis |
|---|---|
| Resource usage for billing | Hypervisor instrumentation |
| Network control | Host vNIC |
| The actor in the audit log | The Control Plane's lease ledger |
| Quota enforcement | Control Plane |

The heartbeat's `load` is an observability hint, not a basis for billing or scheduling.

### 19.6 Guest ↔ stream gateway media contract

§6.3 fixed the media parameters but left the transport contract empty. It is filled here.

| Item | Choice | Rationale |
|---|---|---|
| Transport | **SRTP over UDP**, guest → gateway, one-way | The gateway relays as-is to viewers, so no repacketization is needed |
| Path | Host-internal network; never traverses the internet | No ICE required (§6.3) |
| Authentication | An SRTP key issued at Desktop provisioning, with initial registration via the guest mTLS certificate | The gateway accepts only the SSRC of a registered Desktop |
| Layer id | **RTP extension header** (a one-byte extension in the style of `urn:3gpp:video-orientation`) | The gateway drops layers by header alone, without opening the payload |
| Still overlay | Sent **over the control stream rather than the media path** (§19.5 `StreamChunk`), then relayed by the gateway to the viewer's DataChannel | WebP cannot ride the video track, and the gateway does not decode, so it only passes it through |

The gateway **never decodes anything.** Layer dropping is header-based, the overlay is passed through, and masking has already been applied in the guest — none of the three requires the transcoding §6.3 rejected.

### 19.7 Everything else

| Area | Choice |
|---|---|
| Metadata database | PostgreSQL |
| Events and queues | NATS or Redis Streams |
| Object storage | S3-compatible (screenshots, recordings, snapshots) |
| Orchestration | Kubernetes for the Control Plane, plus a bespoke Firecracker scheduler for Desktop placement, affinity, and snapshot management |
| Observability | OpenTelemetry → Prometheus / Loki / Tempo |
| Streaming | WebRTC by default, WebSocket JPEG-diff as fallback |

---

## 20. Glossary

| Term | Definition |
|---|---|
| **Desktop** | The persistent virtual computer **co-owned** by a human and an agent |
| **Session** | A control connection held by a human or an agent. Many may exist at once; exactly one holds `WRITE` |
| **Computer API** | The OS-agnostic unified control interface |
| **`iapetusd`** | The Iapetus daemon running inside the guest OS. This name is used throughout, distinct from "agent" |
| **OWNER mode** | The default mode in which both humans and agents hold root/Administrator authority over the Desktop |
| **restricted mode** | An opt-in mode enforcing an application allowlist and shell blocking |
| **App catalog** | The list of launchable applications. A discovery tool, not a blocking mechanism |
| **Owner** | A principal with full authority over a Desktop, either `human` or `agent` |
| **Control lease** | The exclusive right to send input. One per Desktop |
| **Preempt** | A human immediately reclaiming the control lease from an agent |
| **Suspend** | Sleeping after a memory snapshot. Whether processes survive depends on the runtime (§7.4) |
| **Project Key** | The long-lived secret held by the customer's server, used to issue tokens and manage resources (§8.1) |
| **Agent Token / Viewer Token** | Short-lived tokens scoped to specific Desktops |
| **Frame Source** | The guest's single capture ring buffer, shared by the still and video encoders (§6.3) |
| **Stream Gateway** | The SFU component that replicates an encoded stream to many viewers |
| **Fallback** | The lower-quality path that switches to WebSocket JPEG diff where WebRTC is unavailable |
| **crypto-shredding** | Deletion by destroying the encryption key so the data cannot be decrypted. Iapetus destroys the key **at T+24h**, after the 24-hour recovery grace (§10.3) |
| **DEGRADED** | A state of limited functionality caused by daemon protocol mismatch, Guest Token renewal failure, and similar. It recovers on the next resume; a Desktop that never resumes is flagged after 7 days and restarted in a maintenance window after 30 (§19.4) |
| **Kill criteria** | The pre-agreed conditions for stopping or pivoting the product (§17.3) |
| **Observation ratio** | The fraction of ACTIVE Desktops a human is watching over a stream. The key variable in capacity planning (§12.4) |
