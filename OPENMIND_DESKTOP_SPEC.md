# OpenMind Desktop — Architecture Spec

**A sharper, fully-local competitor to OpenHuman. Desktop-first.**

Status: design spec, not yet built.
Distinct from: OpenMind CLI (Python, existing), OpenMind Lite (mobile/PWA spec).

---

## 1. The actual competitive gap

OpenHuman is good. Rust+Tauri core, Memory Tree (auto-fetched, compressed,
Obsidian-compatible local vault), TokenJuice compression, a Subconscious
background loop, 118+ integrations via Composio, 27k+ stars, real users.
Beating it on feature count is not realistic and not the right fight.

**The gap worth attacking is the one place OpenHuman's own marketing
contradicts its implementation**: it's pitched as "local-first, private,"
but by the project's own architecture docs, default chat, vision, web
search, integration OAuth proxying, and TTS streaming all route through
OpenHuman's hosted backend. Memory storage is local. The thinking isn't.

That gap is the entire wedge:

> **OpenMind Desktop: every claim "local-first" makes is literally true.
> No backend. No proxy. No exceptions. Same integration breadth, same
> background intelligence, same compression discipline — with nothing
> that ever leaves the machine.**

This is a harder engineering problem (you don't get to lean on a hosted
model router or a managed OAuth proxy) but it's a real, defensible,
honestly-claimable difference, not a marketing angle.

---

## 2. Stack decision

Match OpenHuman's stack choice — it's the correct one, not worth
relitigating:

```
Tauri v2 (shell) + Rust (core) + React/TypeScript (frontend)
```

- **Rust core**: business logic, tool execution, memory pipeline,
  compression, scheduling. Same reasons OpenHuman chose it over a
  Python/Electron stack — lower memory footprint, faster startup,
  no separate runtime to bundle, safer concurrency for the background
  loop.
- **Tauri over Electron**: smaller binary, lower idle memory, no bundled
  Chromium-per-app overhead.
- **React frontend**: presentation only, talks to the Rust core over a
  local IPC/RPC boundary — same separation OpenHuman uses, because it's
  the right separation (UI changes shouldn't touch business logic).

The existing Python OpenMind CLI's *ideas* (LazyAgent, MemorySystem,
TemplateRouter) port over as architecture, not as code — this is a
ground-up Rust implementation, not a Python-to-Rust transliteration.

---

## 3. Core differentiator #1 — fully local inference, no exceptions

Every model call — chat, summarization, the background loop, tool
selection — goes through a **local model router** with zero hosted
fallback by default:

```
┌────────────────────────────────────────────┐
│  Local Model Router                          │
│  • Ollama (primary)                          │
│  • LM Studio (alternative)                   │
│  • llama.cpp embedded (zero-config fallback) │
└────────────────────────────────────────────┘
```

- **Zero-config path**: bundle a small embedded llama.cpp runtime so the
  app works immediately on first install with no external server setup
  — OpenHuman's "context within minutes" UX, without the hosted-backend
  dependency that earns that speed.
- **Bring-your-own-model**: point at Ollama/LM Studio for bigger local
  models when the user has the hardware.
- **No cloud model option in v1.** Not "off by default" — genuinely
  absent from the codebase. The moment a cloud toggle exists, the "every
  local-first claim is literally true" wedge is gone. If demand for an
  optional cloud model shows up later, it ships as a clearly-labeled,
  separate, opt-in module — never the default path.

---

## 4. Core differentiator #2 — LazyAgent vs. TokenJuice

TokenJuice compresses tool *outputs* before they reach the model (HTML→
Markdown, dedup, summarize) — real, useful, ~80% claimed reduction on
tool-call payloads specifically.

LazyAgent's gate-before-you-even-build-a-request approach is a strictly
larger surface:

| | TokenJuice (OpenHuman) | LazyAgent (OpenMind Desktop) |
|---|---|---|
| Compresses tool outputs | ✅ | ✅ (port the idea) |
| Skips the LLM call entirely for repeated/cacheable queries | ❌ | ✅ exact + semantic cache |
| Skips the LLM call entirely for trivial intents (greetings, status) | ❌ | ✅ rule engine |
| Compresses memory context before generation, not just tool output | unclear from docs | ✅ |

Same four-layer pipeline as desktop OpenMind (gate → cache → compress →
act), reimplemented in Rust, applied uniformly to chat, the background
loop, and tool calls — not just tool-call payloads.

---

## 5. Core differentiator #3 — Memory Tree, matched and made fully local

Match the actual good idea here — auto-fetched, compressed, human-
readable local knowledge base — without the backend dependency:

- **Local Memory Tree**: same shape as OpenHuman's — hierarchical,
  compressed, Markdown-based, Obsidian-vault-compatible (don't reinvent
  a good interop choice; users already have Obsidian workflows).
  Genuinely local-only diff: chunking/summarization runs through the
  *local* model router (§3), not a hosted one.
- **Background loop** ("Subconscious," matched conceptually): runs on a
  schedule (default 20 min, matching the bar already set, configurable),
  re-indexes new data, updates the tree, surfaces patterns — entirely
  via the local model, entirely on-device.
- **Storage**: SQLite (matches OpenHuman's own choice — proven, fine,
  not a place to innovate) + the Markdown vault as the human-readable
  layer on top.

---

## 6. Core differentiator #4 — integrations via MCP, not a bespoke system

This is the resolved question from the kickoff discussion: **build a
generic connector framework, not a fixed integration list.** The
concrete decision is to build it MCP-native rather than inventing a
proprietary adapter format:

```
┌──────────────────────────────────────────────┐
│  Integration Framework (Rust core)             │
│  • MCP client (stdio + Streamable HTTP)        │
│  • Connector manifest format (auth, schema,     │
│    fetch cadence, output → Memory Tree mapping) │
│  • Auto-fetch scheduler (same cadence model      │
│    as the background loop)                      │
└──────────────────────────────────────────────┘
            │
            ├─ Any existing MCP server works immediately
            │  (Anthropic's connector directory, Composio's
            │  catalog, anything self-hosted)
            │
            └─ OpenMind-specific connector manifests for
               popular services where a thin wrapper adds
               value (auth UX, fetch-cadence defaults,
               Memory Tree chunk mapping)
```

**Why MCP over a Composio-style bespoke system**: MCP is the standard
the ecosystem has already converged on. Building OpenHuman's
118-integration breadth from scratch is the single highest-effort,
highest-risk part of matching them — but building it *MCP-native*
means OpenMind inherits the existing MCP server ecosystem for free
instead of writing and maintaining N service-specific parsers in-house.
This is the actual leverage point: comparable or greater eventual
breadth, a fraction of the maintenance burden, and it's the
architecturally correct choice independent of competitive
positioning.

**OAuth handling**: unlike OpenHuman, which proxies OAuth flows through
its backend, OpenMind Desktop performs OAuth redirects locally
(loopback redirect URI, standard desktop-app OAuth pattern) and stores
tokens encrypted on-device only. Slower to bootstrap per-connector than
"OpenHuman's hosted proxy handles it for you," but it's the same
local-only claim made consistently rather than carved out for the one
piece (auth) that's hardest to do without a backend.

**v1 scope**: ship the framework + a small number of first-party
connector manifests (Gmail, Notion, GitHub, Slack, Calendar — the ones
OpenHuman itself leads with) done well, then let the MCP ecosystem
supply breadth. Don't attempt to hand-build 118 connectors before
shipping anything.

---

## 7. Core differentiator #5 — Voice, fully local

OpenHuman's voice layer is push-to-talk dictation in, ElevenLabs TTS out,
plus a live Google Meet agent that joins as a participant. The STT/TTS
half is genuinely good UX; the dependency is the issue: ElevenLabs is a
hosted API, meaning every spoken word leaves the machine the moment
voice is used, in an app whose entire pitch is "your data never leaves
your machine."

**Local equivalent:**

```
┌──────────────────────────────────────────────┐
│  Voice Pipeline (local)                        │
│  STT: whisper.cpp (same embedding family as     │
│        the local model router's llama.cpp path) │
│  TTS: Piper or a local Coqui-derived model       │
│  Hotkey: OS-level push-to-talk, same Accessibility│
│          /Input Monitoring permission model as   │
│          OpenHuman uses                          │
└──────────────────────────────────────────────┘
```

- **STT**: `whisper.cpp` is the obvious choice — same C++/Rust-bindable
  shape as the llama.cpp path already in the Local Model Router (§3),
  so it shares the "embedded, zero-config, works on first install"
  property rather than introducing a second installation story.
- **TTS**: this is the harder half. ElevenLabs' quality is good
  specifically because it's a large hosted model; local TTS (Piper,
  Coqui) is real and runnable but a genuine quality step down,
  especially on weaker hardware. **Be upfront about this tradeoff in
  product copy rather than overselling it** — "fully local" and
  "matches ElevenLabs quality" are not simultaneously true claims to
  make.
- **Meeting agent (joining Google Meet as a participant)**: explicitly
  out of scope for the local-first version at v1. This needs either a
  browser-automation layer or Meet's API, and realistically depends on
  cloud STT for live multi-speaker transcription quality — there isn't
  a credible fully-local equivalent yet. Flag as a deliberate gap rather
  than attempting a worse, hosted-anyway version that undermines the
  "no exceptions" claim from §1.

**Permissions model**: matches OpenHuman's own approach — OS prompts for
Microphone (and Input Monitoring on macOS for the hotkey) on first
voice use, reviewable later in settings. This part isn't a place to
diverge; it's already the right UX.

**Real risk**: whisper.cpp's accuracy at small model sizes (the ones
that don't blow the install-size budget set in §3) is noticeably behind
hosted STT. This is the same quality-vs-local tradeoff that runs through
the whole project — name it, don't hide it.

---

## 8. Core differentiator #6 — Screen Intelligence, fully local

OpenHuman's Screen Intelligence captures the active window every few
seconds, summarizes it, and feeds that into the agent's context — "what
was I working on before the standup?" becomes answerable. Per-app
permissions let the user control what's captured. This is a strong,
genuinely differentiated feature (distinct from the Memory Tree's
external-data ingestion — this is *local activity* awareness) and one
of the more defensible reasons to use a desktop agent over a browser
tab.

The architecture question here isn't proxy-vs-local the way voice and
OAuth are — OpenHuman's own marketing states screen summarization is
processed locally already. The risk surface for OpenMind Desktop is
different: **doing this responsibly at all**, regardless of where the
model runs.

```
┌────────────────────────────────────────────────┐
│  Screen Intelligence Pipeline                     │
│  Capture: OS screenshot API, active window only    │
│           (not full multi-monitor by default)      │
│  Summarize: local vision-capable model (see §3 —    │
│             this is what the model router's "vision"│
│             workload tier is for)                   │
│  Store: short-lived raw capture, discarded after     │
│         summarization; only the text summary persists│
│         into the Memory Tree (§5)                     │
│  Gate: per-app allow/block list, off by default        │
│        until explicitly enabled                         │
└────────────────────────────────────────────────┘
```

- **Capture scope**: active window only, not the full desktop, by
  default — narrower than it has to be, on purpose. Multi-monitor/
  full-desktop capture is a meaningfully larger privacy surface and
  shouldn't be the default even if it's offered as an opt-in later.
- **Raw screenshots are not retained.** The image is summarized into
  text by a local vision model and then discarded — only the summary
  enters the Memory Tree. This is a stronger privacy stance than
  "stored locally," which OpenHuman can claim but a raw-screenshot
  store still represents real risk if the machine itself is
  compromised. Don't keep what you don't need.
- **Per-app gating, off by default.** OpenHuman ships this as a
  permission control; making it default-off rather than default-on
  with an opt-out is the more defensible posture for a privacy-focused
  competitor to take, even though it costs some of the "instant value"
  onboarding magic.
- **This needs a real local vision model**, which is a heavier
  dependency than the text-only model router described in §3. This
  should be treated as a separate, optional model download — not
  bundled into the zero-config embedded path, since most sessions
  won't need it and it meaningfully grows install size.

**Real risk**: this is the single most privacy-sensitive feature in the
entire app, by a wide margin — it's continuous passive capture of
whatever the user is looking at, which can include other people's
private messages, financial data, anything visible on screen regardless
of whether it's "yours." Ship default-off, ship a clear and persistent
indicator whenever it's active (not just a settings toggle buried in a
menu), and treat per-app gating as a hard requirement for v1 of this
feature specifically, not a nice-to-have — unlike most v1 scope calls
elsewhere in this spec, this is not a place to cut corners to ship
faster.

---

## 9. Core differentiator #7 — Multi-agent orchestration, fully local

OpenHuman's "intelligence layer" runs steerable async sub-agents,
durable workflow orchestration with an approval gate for high-cost
runs, and worktree isolation for complex multi-step tasks running
concurrently. This is real, substantial agent-orchestration
infrastructure — not a marketing term — and is the part of OpenHuman's
architecture furthest from "simple chat app."

**Local equivalent, scoped honestly:**

```
┌──────────────────────────────────────────────────┐
│  Orchestrator                                       │
│  - Owns the task queue, talks to LazyAgent (§4) per   │
│    sub-agent so the gate/cache/compress pipeline       │
│    applies uniformly, not just to single-turn chat       │
│  - Approval gate: any sub-agent run estimated above a     │
│    cost/time threshold pauses for explicit user sign-off   │
├──────────────────────────────────────────────────┤
│  Sub-agents (steerable, async)                       │
│  - Each runs against the Local Model Router (§3)        │
│  - Can be interrupted/redirected mid-run, not just         │
│    fire-and-forget                                           │
├──────────────────────────────────────────────────┤
│  Worktree isolation                                   │
│  - Filesystem-touching sub-agents (the MCP connector       │
│    framework's tool calls, §6) operate in an isolated        │
│    scratch directory, merged back only on completion          │
└──────────────────────────────────────────────────┘
```

- **Approval gate for high-cost runs**: this maps directly onto an
  honest local-compute equivalent — "cost" isn't API dollars here, it's
  estimated local compute time / battery impact, surfaced the same way.
  Worth keeping the UI pattern (explicit approval before a long-running
  agent task starts) even though the underlying cost model is
  different.
- **Worktree isolation matters more locally, not less.** OpenHuman
  isolates concurrent runs to avoid cross-contamination between agent
  tasks; for OpenMind Desktop, a sub-agent with MCP tool access (§6)
  touching the local filesystem is a real safety boundary, not just a
  cleanliness feature — isolate by default.
- **Steerable async sub-agents** is the most complex single piece of
  this entire spec to actually build. It requires real concurrent task
  management, cancellation that's cooperative (not abrupt — OpenHuman's
  own changelog specifically calls out hardening cancellation to be
  "more cooperative and resilient under load," which is a signal this
  is genuinely hard to get right, not a solved problem they breezed
  past).

**Honest scoping call**: this is explicitly the last differentiator to
build, not the first. Everything else in this spec (model router,
LazyAgent, memory, MCP, voice, screen intelligence) is meaningfully
useful as a single-agent, single-threaded experience. Multi-agent
orchestration is a multiplier on top of a working foundation, not a
replacement for one — attempting it before the foundation is solid
risks building orchestration infrastructure around features that don't
exist yet to orchestrate.

---

## 10. Core differentiator #8 — Messaging channel agents, fully local credentials

OpenHuman treats Telegram and Discord as first-class "talk back" surfaces,
not just chat backends — Telegram specifically is positioned as the
primary remote-control channel, with 80+ bot actions (send/receive,
manage chats, search history, create groups) and an active roadmap
(tracked in their own GitHub issue #1805) toward full remote-control
parity: list/start/resume/detach/abort sessions, inline approve/deny
permission requests, model switching, and scheduled tasks — all from
Telegram, away from the desktop entirely.

This is a genuinely good idea independent of the privacy framing — "away
from keyboard" supervision of a long-running agent task is a real use
case. The local-first question here is narrower than voice or OAuth:
it's about **where the bot credentials live**, not where the model runs.

```
┌──────────────────────────────────────────────────┐
│  Messaging Channel Agents                            │
│  Telegram: two-way, remote-control surface             │
│    - Own bot token (via @BotFather) stored locally,      │
│      encrypted — no "connect via OpenMind" managed         │
│      credential mode, by design (§1: no backend, no        │
│      exceptions, so there is no managed mode to offer)      │
│  Discord: send/receive, same local-credential model           │
│  Web: in-app local chat (already covered by the React          │
│        frontend itself — not a separate channel to build)        │
└──────────────────────────────────────────────────┘
```

- **Bot token handling**: OpenHuman offers two modes — one-click via
  their backend, or bring-your-own-token. OpenMind Desktop only has the
  second mode, consistent with §1's "no exceptions" rule — there's no
  backend to broker a one-click flow through. This is real friction
  (BotFather setup is a few extra manual steps) traded for the token
  never touching anything but the user's own machine.
- **Remote-control surface, not just relay**: matching the ambition of
  OpenHuman's #1805 roadmap rather than just message-in/message-out —
  list/start/resume/abort sessions, approve/deny permission requests
  (ties directly into the Orchestrator's approval gate from §9) inline
  from Telegram, status view of current session/model/active task.
- **A specific bug to design around, not just match**: OpenHuman's own
  issue tracker (#1948) documents duplicate "operator approval required"
  prompts firing 2-4 times per message, traced to multiple handlers
  processing the same inbound update in parallel without a shared,
  consistently-read source of truth for the allowed-users list. The
  direct lesson for this design: **the channel runtime needs exactly
  one code path that resolves "is this sender authorized," shared by
  every handler that checks it** — not independently-maintained checks
  in each message-handling branch that can drift out of sync with each
  other.
- **Keep transport generic.** OpenHuman's own roadmap explicitly calls
  this out as a design goal worth copying: expose capabilities through
  a channel-runtime abstraction, not Telegram-specific branches
  scattered through the Orchestrator (§9) and Connector framework (§6)
  — Discord (and any future channel) should plug into the same
  controller registry rather than needing its own parallel
  implementation.

**Real risk**: scope discipline. "Remote control surface" can expand
indefinitely (OpenHuman's own roadmap issue lists model switching,
scheduled tasks, live status views, inline approvals as separate
sub-features). Treat two-way send/receive plus the approval-gate
integration as the actual v1 bar; the rest of #1805's wishlist is real
but is its own future milestone, not a blocker for shipping channel
support at all.

---

## 11. Core differentiator #9 — OS-level text autocomplete, fully local

OpenHuman's inline autocomplete is explicitly **not** a browser
extension — it works across any desktop application (email clients,
document editors, code environments) via OS-level accessibility APIs,
and it's memory-aware: suggestions draw on the Memory Tree (§5), not
just local sentence context.

```
┌────────────────────────────────────────────────┐
│  Text Autocomplete                                 │
│  Hook: OS accessibility API (matches the approach    │
│        already used for the voice push-to-talk         │
│        hotkey in §7 — same permission family)             │
│  Context: current field content + relevant Memory Tree      │
│           recall (§5), not just local n-gram completion       │
│  Inference: Local Model Router (§3), routed through            │
│             LazyAgent (§4) like every other generation call      │
└────────────────────────────────────────────────┘
```

- **Same permission model as voice (§7)**: macOS Accessibility/Input
  Monitoring, Windows UI Automation, Linux AT-SPI — this is genuinely
  the same OS-integration family as the push-to-talk hotkey, so it's
  worth building as a shared "OS integration" layer rather than two
  independently-permissioned features that happen to both need
  accessibility access.
- **Routes through LazyAgent like everything else.** This is the
  highest-frequency generation surface in the entire app by volume —
  every few keystrokes is a potential completion request — which makes
  it the single best argument for why LazyAgent's gate/cache layers
  (§4) need to be fast and cheap, not just token-efficient. A slow
  rule-engine check here is felt immediately as input lag; this
  feature is a forcing function for keeping that path lightweight.
- **Memory-aware is the actual differentiator over standard OS
  autocomplete/autocorrect** — pulling in relevant Memory Tree context
  (a person's name from a recent email, a project term from Notion) is
  what makes this "AI autocomplete" rather than "spell-check." This
  depends on Memory Tree search (§5) being fast enough for
  per-keystroke or per-pause latency budgets, which is a real
  constraint worth testing early rather than assuming.

**Real risk**: latency budget. Unlike chat (where a second or two of
"thinking" is acceptable UX), inline autocomplete needs to feel
near-instant or it gets disabled by users within minutes of trying it.
This likely means a much smaller/faster model tier than chat uses, or
even a non-LLM local completion model for the lowest-latency tier —
worth treating as a distinct routing target in the Local Model Router
(§3) rather than assuming the same model serves both chat and inline
completion well.

---

## 12. Skills — explicitly scoped against a moving target

OpenHuman ships a "skills" concept — sandboxed modules that fetch
external data, run on a schedule, transform information, and respond to
events. Earlier versions ran skill code through an embedded QuickJS
runtime. **As of the most recent available information, that runtime
has been removed and skill execution is mid-rebuild** — the current
skills surface is metadata-only (discover, parse, install, uninstall),
with no executable third-party skill packages actually running today.

This matters for how this section should be read: **spec'ing a feature
to match OpenHuman's skills system risks copying a system that doesn't
fully exist yet itself.** Rather than design a speculative match to an
unstable target, the honest move is to scope what a skills system would
need architecturally if/when it's worth building, and flag explicitly
that this should be re-checked against OpenHuman's actual shipped state
before committing real engineering time to it.

```
┌────────────────────────────────────────────────┐
│  Skills (design sketch — not a committed feature)    │
│  Catalog: manifest-driven discovery, same shape as       │
│           the Connector Manifest format (§6) — a skill      │
│           is architecturally close to a connector with        │
│           a schedule attached, not a separate concept           │
│  Executor: sandboxed, isolated — NOT QuickJS specifically;       │
│            evaluate WASM-based sandboxing (wasmtime/wasmer)        │
│            given the Rust core already needs WASM tooling          │
│            knowledge for nothing else here, so this would be a       │
│            net-new dependency either way                              │
│  Trigger: scheduled (cron-like) or event-driven (e.g. a                │
│           connector fetch completing), feeding into the                 │
│           Orchestrator's task queue (§9) rather than being a              │
│           fourth, separate execution path                                  │
└────────────────────────────────────────────────┘
```

- **Architecturally, this is "a connector plus a schedule plus a
  sandbox,"** not a fourth independent system. If built, it should
  reuse the Connector Manifest format from §6 rather than inventing a
  parallel manifest shape.
- **Sandboxing is the part that can't be skipped or rushed.** Whatever
  OpenHuman's QuickJS removal was about, running arbitrary third-party
  code (even "small" transform scripts) inside a desktop app with
  filesystem/MCP tool access is a real attack surface, not a detail —
  this is the same posture as the Orchestrator's worktree isolation in
  §9 applied to a different execution context.
- **Do not build this before re-verifying OpenHuman's actual current
  skills implementation.** Most other sections in this spec are stable
  enough to design against confidently; this one specifically is
  explicitly a moving target as of this writing, and the right call
  might be "match whatever they land on" rather than "build ahead of
  them" — worth a fresh check before any engineering time goes here.

---

## 13. What this looks like end to end

```
┌───────────────────────────────────────────────────────┐
│  Tauri shell                                              │
│  windowing, OS integration, tray, native notifications     │
├───────────────────────────────────────────────────────┤
│  Rust core                                                 │
│  ┌─────────────────────────────────────────────────────┐  │
│  │  Orchestrator (§9)                                      │  │
│  │  task queue, approval gate, worktree isolation            │  │
│  └─────────────────────────────────────────────────────┘  │
│  ┌─────────────┐ ┌──────────────┐ ┌─────────────────┐     │
│  │ LazyAgent    │ │ Memory Tree   │ │ MCP Integration  │     │
│  │ gate→cache→  │ │ pipeline +    │ │ Framework        │     │
│  │ compress→act │ │ Background    │ │ (client + fetch  │     │
│  │ (§4)         │ │ loop (§5)     │ │  scheduler) (§6) │     │
│  └─────────────┘ └──────────────┘ └─────────────────┘     │
│  ┌─────────────┐ ┌──────────────────────────────────┐     │
│  │ Voice (§7)   │ │  Screen Intelligence (§8)            │     │
│  │ whisper.cpp  │ │  active-window capture → local        │     │
│  │ STT, local   │ │  vision summary → discard raw image,    │     │
│  │ TTS          │ │  off by default, per-app gated            │     │
│  └─────────────┘ └──────────────────────────────────┘     │
│  ┌─────────────┐ ┌──────────────────────────────────┐     │
│  │ Messaging    │ │  OS Text Autocomplete (§11)          │     │
│  │ Channels(§10)│ │  shares the accessibility-API hook      │     │
│  │ Telegram/    │ │  with Voice (§7); routes through         │     │
│  │ Discord, own │ │  LazyAgent (§4) on the fast/low-latency    │     │
│  │ bot tokens   │ │  path — see §11's latency note              │     │
│  └─────────────┘ └──────────────────────────────────┘     │
│  ┌─────────────────────────────────────────────────────┐  │
│  │  Local Model Router (§3)                                  │  │
│  │  embedded llama.cpp (zero-config) │ Ollama │ LM Studio       │  │
│  └─────────────────────────────────────────────────────┘  │
│  SQLite + Markdown vault (Obsidian-compatible)              │
├───────────────────────────────────────────────────────┤
│  React frontend — presentation only, talks to core          │
│  via local IPC/RPC                                           │
└───────────────────────────────────────────────────────┘
```

Skills (§12) deliberately don't appear as a box here — per §12, that
section is a design sketch against a target that's still moving on
OpenHuman's own side, not a committed piece of this architecture yet.

No box in this diagram ever talks to anything outside the machine
except: (a) the MCP connectors the user explicitly authorizes, fetching
*their own* data from *their own* accounts, (b) Telegram/Discord's own
APIs, authenticated with the user's own bot credentials (§10), and (c)
downloading model weights once, on first install or first use of an
optional capability (voice, vision) — consistent with §3's "no cloud
model option" rule: every model involved, including STT/TTS/vision,
runs locally or it isn't in the product.

---

## 14. Positioning summary

| | OpenHuman | OpenMind Desktop |
|---|---|---|
| Stack | Rust + Tauri | Rust + Tauri (matched — it's correct) |
| Local memory storage | ✅ | ✅ |
| Default chat/search/voice routing | Hosted backend | Local model router, no exceptions |
| OAuth handling | Backend proxy | Local loopback, on-device token storage |
| Compression | TokenJuice (tool outputs) | LazyAgent (tool outputs + whole-request gating + cache) |
| Integration model | Composio-managed catalog, 118+ | MCP-native framework + first-party manifests, ecosystem-extensible |
| Background intelligence | Subconscious Loop | Matched, fully local |
| Voice | ElevenLabs TTS (hosted), Meet agent | whisper.cpp STT + local TTS (lower quality, named honestly); Meet agent out of scope v1 |
| Screen Intelligence | Local processing claimed, raw capture retention unclear | Local vision model, raw screenshot discarded after summarization, off by default |
| Multi-agent orchestration | Steerable async sub-agents, approval gate, worktree isolation | Matched design, scoped as last-built (foundation-dependent) |
| Messaging channels | Telegram (managed or own-token), Discord | Telegram/Discord, own bot tokens only — no managed mode, by design |
| OS-level autocomplete | Memory-aware, accessibility-API based | Matched design, shares Voice's permission layer (§7) |
| Skills | Metadata-only as of writing; execution mid-rebuild | Explicitly not committed (§12) — re-check before building |
| Claim-to-implementation gap | "Local-first" with hosted defaults | None — every local claim is literally true |

---

## 15. Open questions before build starts

1. **Embedded model choice for zero-config path** — needs a model small
   enough to bundle reasonably (install size matters for adoption) but
   capable enough that first-run quality doesn't undersell the product.
2. **Background loop cadence** — match OpenHuman's 20-minute default, or
   is there a better-justified interval given this runs fully on local
   compute (battery/CPU impact differs from a backend-scheduled job)?
3. **First-party connector priority order** — Gmail/Notion/GitHub/
   Slack/Calendar matches OpenHuman's lead list; confirm that's the
   right set for the target user, or adjust before building manifests.
4. **OAuth UX for the loopback-redirect pattern** — needs a concrete
   design pass; "local OAuth is slower to bootstrap" is a real UX cost
   that needs a mitigation plan, not just an architectural footnote.
5. **Local TTS quality bar** — at what point is local TTS quality "good
   enough to ship" vs. a feature that undersells the product relative
   to OpenHuman's ElevenLabs-backed voice? Needs a real side-by-side
   test, not a guess, before committing to a specific local TTS engine.
6. **Vision model size for Screen Intelligence** — same shape as open
   question #1 but for a heavier, optional dependency; needs its own
   answer since it shouldn't be bundled into the always-installed
   zero-config path.
7. **Bot setup UX for Messaging Channels (§10)** — without a managed
   one-click mode, BotFather/Discord developer-portal setup is real
   friction for non-technical users; needs a guided-but-still-local
   setup flow, not just documentation telling people to go read
   Telegram's bot docs.
8. **Autocomplete latency budget, measured not assumed (§11)** — what's
   the actual acceptable response time before users disable the
   feature? This should be tested early, since it determines whether
   inline autocomplete needs its own dedicated fast model tier in the
   Local Model Router (§3) rather than sharing the chat-tier model.
9. **Skills — re-verify before scoping further (§12)** — this entire
   section is explicitly provisional; before any engineering time goes
   toward it, check OpenHuman's current shipped skills implementation
   again, since it was mid-rebuild as of this spec's writing and may
   have landed somewhere specific by the time this milestone is
   reached.

---

## 16. Suggested build order

1. Tauri shell + Rust core skeleton, IPC boundary to a placeholder React UI
2. Local Model Router: embedded llama.cpp zero-config path first (this
   is what makes first-run work without external setup)
3. Port LazyAgent (gate → cache → compress → act) into the Rust core
4. SQLite + Markdown vault storage layer, basic Memory Tree (no
   auto-fetch yet — manual ingestion only)
5. MCP client + connector manifest format, prove it against one
   existing public MCP server before writing any first-party manifest
6. Background loop (auto-fetch scheduler + re-indexing), starting with
   the one connector from step 5
7. First-party connector manifests: Gmail → Notion → GitHub → Slack →
   Calendar, in that order, each shipped and validated before the next
8. Voice: whisper.cpp STT first (shares the llama.cpp embedding
   pattern from step 2), local TTS second, after open question #5 is
   answered with real testing
9. OS-level text autocomplete: builds directly on step 8's
   accessibility-API integration work — do this right after voice while
   that permission-handling code is still fresh, not as a separate
   later effort. Settle open question #8 (latency budget) before
   considering this done.
10. Screen Intelligence: behind a default-off flag from day one, per-app
    gating is a launch requirement for this specific feature (not
    deferrable like most v1 scope calls elsewhere in this list)
11. Multi-agent orchestration: deliberately last among the core/already-
    designed features — everything above this line should be solid and
    in real use before building an orchestrator to coordinate it
12. Messaging channel agents (Telegram/Discord): depends on step 11's
    approval-gate plumbing for the inline-approve/deny capability to
    mean anything; build the shared-authorization-check design from §10
    before adding a second channel, not after (this is the direct
    lesson from OpenHuman's own #1948 bug)
13. OAuth loopback flow, encrypted local token storage
14. Skills: only after re-checking open question #9 — this may mean
    "build it" or "wait and match whatever OpenHuman lands on," and
    that decision should be made fresh at this point, not locked in now
15. Real comparative testing against OpenHuman on the same machine —
    memory footprint, startup time, voice/screen-intelligence/
    autocomplete quality gaps honestly measured, and the actual
    local-vs-cloud claim, not just feature checklist parity
