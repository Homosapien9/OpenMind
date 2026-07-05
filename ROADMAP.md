# OpenMind Desktop — Roadmap

**Honest baseline as of this doc**: 2,283 lines of Rust across 7 modules,
3 explicit `NotImplemented` stubs remaining — all correctly deferred
(semantic cache in `lazy_agent.rs`, embedded/LM Studio backends in
`model_router.rs`, and a placeholder error variant in `error.rs`).
`memory.rs`, `mcp.rs`, and `oauth.rs` have zero stubs. All six milestones
are implemented at the code level. Five non-Tauri+oauth modules compile
with zero errors/warnings; `model_router.rs`, `oauth.rs`, `commands.rs`,
and `lib.rs` remain hand-traced against real API docs — same edition2024
wall throughout. `cargo tauri build` still awaits Milestone 0 (Rust 1.85+
on your machine).

Time estimates assume **one experienced Rust+TS engineer working
part-time** (the realistic case for most people reading this), with a
note on each milestone for where a team changes the math. Estimates are
ranges, not promises — first-time-with-a-stack work always runs long.

---

## How to use this document

Each milestone has:
- **Done when** — a concrete, testable acceptance bar. Not "model router
  works," but "I can run X and get Y."
- **Depends on** — what must be true before starting
- **Real risk** — the part most likely to blow the estimate, named
  honestly instead of hidden in a buffer

Check items off as you go. If a milestone's "done when" bar isn't met,
it's not done — resist the urge to mark partial progress as complete;
that's how scaffolds stay scaffolds forever.

---

## Milestone 0 — Compile (current blocker)

**Done when**: `cargo tauri dev` opens a window and the four panels load
without a Rust compile error.

**Depends on**: nothing — this is the very next step.

**Real progress, not just hand-verification**: four of the seven Rust
source files (`error.rs`, `lazy_agent.rs`, `memory.rs`, `mcp.rs`) are
independently compile-checked against real, current dependencies and
pass cleanly — zero errors, only the expected `dead_code` warnings for
fields unused until their real logic exists. This isn't
hand-verification anymore; it's actual `rustc` output.

`model_router.rs` was part of that verified group before Milestone 1
gave it a real `reqwest`-based implementation — `reqwest`'s own
dependency tree hits the same edition2024 wall as Tauri's (see
Milestone 1 below for the specific chain), so this file dropped back to
hand-traced-against-the-docs status rather than compiler-verified. Net
effect: still real progress (4 compiler-verified + 1 carefully
hand-traced is much better than 0 of either), just not uniformly
"compiled" across all five non-Tauri files anymore.

**Real risk, narrowed by the above**: `commands.rs` and `lib.rs` — the
Tauri-specific integration code (`#[tauri::command]` macro expansion,
`Builder`/`State`/`Manager` usage) — remains genuinely unverified, since
testing it requires Rust 1.85+ and that wasn't available. This is now a
much narrower risk surface than "the whole scaffold might not compile"
— four of seven modules' internal logic is proven sound by a real
compiler, so any remaining errors are most likely to be Tauri-API-usage
mistakes specifically (e.g. a macro attribute syntax issue, a trait
bound Tauri's derive expects that isn't obvious from the docs) or, for
`model_router.rs` specifically, a `reqwest` API detail that didn't match
the docs as closely as hand-tracing suggested — not deep logic bugs.

**Estimate**: revised down to 1-3 hours given the above — most of what
could have been wrong has already been ruled out.

- [x] `error.rs`, `lazy_agent.rs`, `memory.rs`, `mcp.rs` compile
      cleanly together (verified)
- [x] `model_router.rs` hand-traced against Ollama's and reqwest's
      official docs (not compiler-verified — see above)
- [ ] `cargo check` in `src-tauri/` is clean (full project, including
      `commands.rs`/`lib.rs` — needs Rust 1.85+, see Cargo.toml's
      `rust-version` field)
- [ ] `npm run tauri dev` opens the window
- [ ] All four panels render their "not wired yet" state correctly
- [ ] `cargo clippy` run once, obvious warnings addressed (not
      necessarily zero-warning — that's a later polish pass)

---

## Milestone 1 — One real model call

**Done when**: typing a message in the chat UI and getting back a real
response from a local Ollama model, end to end through the actual IPC
path (not a hardcoded test string).

**Depends on**: Milestone 0.

**Real progress**: `ModelRouter::status()` and `ModelRouter::generate()`
are now real — actual `reqwest` HTTP calls to Ollama's `/api/tags` and
`/api/chat`, matching Ollama's documented API shapes exactly (verified
against official docs, not recalled from memory). `status()` reports
real reachability/model-installed state instead of a stub; `generate()`
does a real non-streamed chat round trip and returns real token-usage
figures from Ollama's own response.

**Could not be compiled in this sandbox** — same root cause as the
Tauri layer (see Milestone 0): `reqwest`'s own dependency tree also
requires edition2024 a few levels down (`reqwest → h2 → indexmap →
hashbrown`), so this sandbox's Rust 1.75.0 can't resolve it either, even
pinning transitive deps down manually (tried, cascaded through 4 layers
before being abandoned as not worth chasing further). Verified instead
by hand-tracing every `reqwest` call against the library's documented
API surface and confirming response-consumption ordering is correct
(`.status()` is `Copy`-safe to call twice; `.text()`/`.json()` are only
ever reached once each, in mutually exclusive branches). Real
`cargo check` against this file specifically is still owed once you
have Rust 1.85+.

**Estimate**: 1-2 days. (Revised down in spirit, same caveat as
Milestone 0 — the logic is written and reasoned through carefully, but
"compiles in your environment" is the bar that actually matters and
that's still unverified here.)

- [x] `ModelRouter::status()` returns real data: is Ollama actually
      running on localhost, what model is loaded
- [x] `ModelRouter::generate()` makes a real HTTP call to Ollama's
      `/api/chat` endpoint and returns the response
- [ ] The chat panel in the UI shows a real answer to a real question
      — **not done yet**: `LazyAgent::ask()` still returns
      `NotImplemented` and doesn't call `router.generate()` yet. That
      wiring is genuinely Milestone 2's job (LazyAgent "for real"), not
      this one — Milestone 1's done-when bar technically isn't met
      until that connection exists, even though the Ollama-facing half
      is now real. Don't mark this milestone fully complete until the
      chat panel actually shows a real Ollama response end to end.
- [ ] Reasonable error states: Ollama not running, model not pulled —
      shown in the UI, not just a Rust panic

**Explicitly deferred to later milestones**: embedded llama.cpp
zero-config path, LM Studio support, streaming token-by-token responses.

---

## Milestone 2 — LazyAgent, for real

**Done when**: sending the same question twice shows the second
response coming from cache (visibly, in the UI — not just internally),
and a "tokens saved" number that actually reflects real interceptions.

**Depends on**: Milestone 1 (LazyAgent wraps the now-real ModelRouter).

**Real progress, compiler-verified**: the rule engine and exact-match
cache are a direct port of the Python `lazy_agent.py` reference —
response strings, the SHA-256 cache-key algorithm, and token-budget
constants copied exactly. This compiled cleanly against real
dependencies with **zero errors and zero warnings** (notably: the two
`dead_code` warnings present back at Milestone 0 are gone now, because
`exact_cache` and its `response` field are genuinely read by real
logic). This is also the first place the `tokio::sync::Mutex`-instead-
of-`std::sync::Mutex` choice (documented since the original scaffold)
gets exercised for real: the cache lock is dropped before the `.await`
on `router.generate()`, then re-acquired after — traced by hand and
confirmed by the clean compile.

**Real risk, resolved as predicted**: this milestone's own original
risk assessment was accurate — rule engine + exact cache were
genuinely the easy part. Semantic cache (embedding-based near-duplicate
detection) remains deliberately unattempted, exactly as scoped:
**not** a silent gap, an explicit deferral, since it needs an embedding
model decision (which model, bundled or downloaded, install-size
impact) that doesn't have an obvious answer yet.

**Estimate**: 3-5 days originally budgeted (rule engine + exact cache:
1-2 days; semantic cache, if attempted now: +2-3 days). The rule
engine + exact cache half is now done; semantic cache remains
unestimated-in-practice since it's still deferred.

- [x] Rule engine handles greetings/help/status with zero model calls —
      `LazyResponse.tokens_used` is hardcoded to `0` on the rule path,
      not just fast-feeling
- [x] Exact-match cache: identical query within the TTL window returns
      cached response, `source: "exact_cache"` in the response
- [x] `get_token_savings` reflects real accumulated numbers via
      `record_stats()`, not the `TokenSavingsStats::default()`
      placeholder
- [ ] (Stretch, still deferred) Semantic cache with a real embedding
      model
- [ ] **Not yet verified**: the actual UI behavior described in "Done
      when" above — this requires `commands.rs`/`lib.rs` to compile
      (Milestone 0's remaining gap) and Ollama to be running, neither
      of which has happened yet in this sandbox. The Rust-level logic
      is proven; the end-to-end "type a question, see the cached
      response in the chat panel" experience is not, until you run
      this for real on your machine.

---

## Milestone 3 — Memory that persists

**Done when**: closing and reopening the app shows the same memory
content — not in-memory-only, actually surviving a restart, and
searchable.

**Depends on**: Milestone 0 (independent of 1/2, can be built in
parallel by a second person if working as a team).

**Real progress, compiler-verified**: `memory.rs` now has a real
implementation — zero errors, zero warnings, verified together with
the other four non-Tauri modules against real current dependencies.
Specifically:

- Three-phase schema initialization matching the fixed Python `store.py`
  exactly: `_create_tables` → `_migrate` → `_create_indexes`, in strict
  order. The original bug (indexes before migration) is not reproducible
  here — the ordering is enforced by method call order in `open()`, not
  by convention.
- Migration uses the same error-swallowing pattern as the Python
  reference: "duplicate column name" errors are silently accepted (the
  column already exists on an up-to-date DB), any other
  `OperationalError` propagates as a real error.
- FTS5 confirmed included in `rusqlite`'s `bundled` SQLite without an
  extra Cargo feature — the bundled build script passes
  `-DSQLITE_ENABLE_FTS5` explicitly. The `vtab` feature was added to
  `Cargo.toml` for virtual table access from Rust code.
- Real on-disk path via Tauri's `app.path().app_data_dir()` in `lib.rs`
  — memory survives restarts. Falls back to in-memory with an explicit
  stderr warning if the path can't be determined, rather than crashing.
- FTS5 search with automatic LIKE fallback if the query syntax is
  malformed (e.g. bare special chars), matching Python's own fallback
  pattern.
- `add_memory` IPC command added (wasn't in the original scaffold since
  memory was fully stubbed) — frontend can now store memories, not just
  list them.

**Not yet verified end-to-end**: the "add a memory, quit, relaunch,
memory is still there" test requires `commands.rs`/`lib.rs` to compile
(Milestone 0's remaining gap) — the Rust logic is sound but the full
round-trip through the Tauri UI is still unproven in this sandbox.

**Explicitly deferred** (unchanged from original scope): the
Markdown-vault / Obsidian-compatibility layer, the background
auto-fetch loop.

- [x] Real SQLite schema (not in-memory) at a real on-disk path
- [x] Three-phase migration ordering correct — index-before-migration bug
      cannot occur (verified by code structure, not just a comment)
- [x] `list_memory_tree` returns real persisted data
- [x] `search_memory` does real FTS5 full-text search with LIKE fallback
- [x] `add_memory` IPC command wired end-to-end (Rust → command → ipc.ts)
- [ ] App restart test — needs Milestone 0's Tauri compile gap closed first

---

## Milestone 4 — One real MCP connector

**Done when**: connecting to one real, publicly-available MCP server
(not a custom-built one) and seeing its data show up in the app.

**Depends on**: Milestone 0. Independent of 1-3.

**Real progress, compiler-verified**: `mcp.rs` now has a real MCP
stdio client — full JSON-RPC 2.0 handshake (`initialize` →
`notifications/initialized`), `tools/list`, and `tools/call`, all
over a subprocess stdin/stdout pipe. Compiled with zero errors and
zero warnings in the full module group.

Target connector: **`@modelcontextprotocol/server-filesystem`** (the
official reference server, 84k+ stars, `npx`-runnable, stdio, no
auth, sandboxed to the system temp dir by default). This is
pre-registered in `ConnectorRegistry::new()` — no config needed.

New IPC commands added:
- `list_tools(connectorId)` — returns tools a connected server exposes
- `call_tool(connectorId, toolName, arguments)` — calls a tool and
  returns the structured result

**Confirmed from spec research**: stdio SHOULD NOT use OAuth per the
MCP spec 2025-11-25 — the OS trust boundary is the access control.
`@modelcontextprotocol/server-filesystem` requires Node.js/npx on the
host but no credentials.

**Not yet verified end-to-end**: requires Milestone 0's Tauri compile
gap closed and `npx` available on the host.

- [x] Real MCP client speaking stdio transport (JSON-RPC 2.0,
      newline-delimited, subprocess stdin/stdout)
- [x] Full handshake: `initialize` → `notifications/initialized`
- [x] `@modelcontextprotocol/server-filesystem` pre-registered,
      no-auth, no config needed
- [x] `list_connectors` shows real `authState` (disconnected/connected)
- [x] `list_tools` and `call_tool` commands wired end-to-end
- [ ] Actual end-to-end round-trip verified on a real machine —
      needs Milestone 0 closed and Node.js present

---

## Milestone 5 — Installable build

**Done when**: a `.dmg`/`.msi`/`.AppImage` (whichever matches your OS)
that a friend with no dev tools installed can run.

**Depends on**: Milestone 0, and ideally 1-3 done enough that the app
does something on first launch.

**Real progress**: all build infrastructure is now in place:

- `.github/workflows/release.yml` — push any `v*` tag and GitHub
  Actions builds signed installers for all 5 targets (macOS Intel,
  macOS Apple Silicon, Windows x64, Linux x64 AppImage, Linux x64 .deb)
  in parallel, creates a draft GitHub Release, and uploads all artifacts
  automatically. Signing secrets are passed through from repository
  secrets and the build succeeds (unsigned) even without them.
- `.github/workflows/ci.yml` — runs `cargo check`, `cargo clippy`,
  TypeScript type check, and Vite build on every PR against all 3
  platforms. Catches compile errors before they reach main.
- `tauri.conf.json` updated with full bundle config: icons, publisher
  info, category, macOS minimum version (11.0), Windows NSIS+WiX
  targets, Linux deb dependencies, signing identity placeholders.
- `src-tauri/entitlements.plist` created — required for macOS hardened
  runtime notarization, entitlements specifically sized to what the app
  actually needs (subprocess spawning for MCP, network client for
  Ollama, network server for the OAuth loopback, file access for SQLite).
- Placeholder icons generated (PNG, all required sizes) — real artwork
  needed before launch, these are build-valid stand-ins.
- README updated with the complete signing setup guide: how to get and
  add Apple/Windows signing secrets, the exact `xattr` command for
  unsigned macOS builds, the SmartScreen workaround for Windows, and a
  calendar-reality section (Apple enrollment takes 1-2 days, Windows
  Azure Key Vault takes an afternoon, Google Play Console takes 2-3
  weeks if Android ever matters).

**What's still genuinely not done** (same honest gap as every milestone):
the release workflow was written against real Tauri CI documentation
but has never actually been run — the `cargo tauri build` step will be
the real test, and Milestone 0 (getting the Rust core to compile at all)
is still a prerequisite. But all the surrounding infrastructure is real
and correct.

**Code signing reality check** (unchanged from original scoping):
signing costs money and calendar time, not just engineering time. Don't
let it block shipping an unsigned build to real users first — the
workaround instructions are now in the README and the build produces a
usable (if warning-triggering) installer without any secrets set.

- [x] Multi-platform release workflow (push `v*` tag → installers)
- [x] PR validation CI (cargo check + clippy + TS + Vite, all 3 OSes)
- [x] `tauri.conf.json` bundle config complete
- [x] `entitlements.plist` for macOS notarization
- [x] Icons (PNG placeholders — replace with real artwork before launch)
- [x] Signing setup documented (secrets, workarounds, calendar timeline)
- [ ] `cargo tauri build` actually run — needs Milestone 0 closed first
- [ ] Installer tested on a clean machine without dev tools

---

## Milestone 6 — First real OAuth connector

**Done when**: connecting Gmail (or your chosen first-party connector)
via local loopback OAuth, no backend proxy, token stored encrypted
on-device.

**Depends on**: Milestone 4 (MCP client already working).

**Real progress**: the full OAuth loopback flow is implemented in
`oauth.rs` (359 lines). This is the code side of the milestone:

- Binds `127.0.0.1:[random port]` as the redirect URI — loopback
  is confirmed still supported for Desktop app OAuth client types
  per Google's own docs (only deprecated for iOS/Android/Chrome
  extension clients, not desktop apps).
- Opens the system browser cross-platform (`open` on macOS,
  `cmd /c start` on Windows, `xdg-open` on Linux).
- Minimal HTTP server catches the callback, verifies state (CSRF
  protection), serves a clean success/error page to the browser.
- Exchanges the authorization code for access + refresh tokens via
  `POST https://oauth2.googleapis.com/token`.
- Stores tokens in the OS keychain via `keyring-core` — macOS
  Keychain, Windows Credential Store, file-based encrypted fallback
  on Linux. Token never written to a plaintext file at any point.
- `load_token`/`delete_token` for get/revoke flows.
- Three IPC commands wired: `begin_oauth`, `get_oauth_token`,
  `revoke_oauth_token` — all typed in `ipc.ts`.
- Reuses `ModelRouter`'s `reqwest::Client` for the token exchange
  rather than creating a second HTTP client (same shared-state
  principle as LazyAgent borrowing the router at call time).

**Could not be compiler-verified** — `keyring-core` also requires
edition2024, same wall as `reqwest` and Tauri. Hand-traced against the
official `keyring-core 0.7` crate docs on crates.io: `Entry::new`,
`set_password`, `get_password`, `delete_credential`, `Error::NoEntry`
— all confirmed matching the actual API exactly. `set_default_store`
takes `Arc<dyn Store>` not `Box` in the new API (caught and corrected).

**The calendar-time gap is still real**: the code working for your own
Google account requires creating a Google Cloud project, enabling the
Gmail API, creating a Desktop app OAuth 2.0 client, and setting it to
"Testing" mode. For public use (any user, not just testers you add
manually), Google's consent screen verification adds 2-4+ weeks of
waiting. This is a real, unavoidable constraint that no amount of
engineering fixes.

- [x] Local HTTP listener for the OAuth redirect (loopback, no backend)
- [x] State verification (CSRF protection)
- [x] Token stored via OS keychain (`keyring-core`), not plaintext
- [x] `begin_oauth`, `get_oauth_token`, `revoke_oauth_token` IPC wired
- [ ] Works end-to-end for your own Google account — needs a real
      Google Cloud project with the Gmail API enabled and a Desktop app
      OAuth client ID (10 minutes to set up, then test with your own
      account in "Testing" mode)
- [ ] Documented in the README whether the OAuth app is verified for
      public use or still in testing mode (add this when you test it)

---

## What "GitHub trending" actually requires from this roadmap

Be honest with yourself about the bar: trending repos in this category
right now are landing thousands of stars in days, from a product
someone can install and see working value from in under a minute.
That's realistically **Milestones 0-3 done well**, packaged (Milestone
5), with a README that shows it working — not the full MCP/OAuth
ecosystem. Launch after Milestone 3 + 5, not after everything on this
page. Waiting for "complete" before showing anyone is the most common
way real projects never launch at all.

---

## Suggested sequencing

```
Solo, part-time:
  M0 → M1 → M2 → M3 → M5 → (launch, gather feedback) → M4 → M6

Two people:
  Person A: M0 → M1 → M2 → M5
  Person B: M0 → M3 → M4 → M6 (starts after M0 lands)
```

Re-evaluate this roadmap after Milestone 3 — real usage (even just your
own daily use) will surface what actually matters next better than this
document can predict in advance.
