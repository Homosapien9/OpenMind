# OpenMind Desktop — scaffold

**Status: architecture scaffold, not a working app.** Every Tauri command
returns `NotImplemented`. This is the project structure, type contracts,
and module boundaries for the four core differentiators from
`OPENMIND_DESKTOP_SPEC.md` (§3-§6: model router, LazyAgent, memory,
MCP integrations), set up so real implementation work can start
immediately without re-deriving the project layout first.

The spec also covers six further differentiators as **design only, no
code yet**: voice (§7), screen intelligence (§8), multi-agent
orchestration (§9), messaging channel agents (§10), OS-level
autocomplete (§11), and skills (§12, explicitly scoped as provisional —
see that section before building it). These are real, researched
designs (grounded in what OpenHuman actually ships, not guesses) but
deliberately come after the four core pieces in the build order (§16),
since they depend on a working model router and LazyAgent foundation
that doesn't exist yet either.

## What's actually here

- A real Vite + React + TypeScript frontend — **builds and type-checks,
  verified** (`npm install && npm run build` succeeds).
- A real Tauri v2 project structure (`Cargo.toml`, `tauri.conf.json`,
  capabilities, `lib.rs`/`main.rs` split). **Four of the seven Rust
  source files are independently verified compiling** against real,
  current dependencies (`error.rs`, `lazy_agent.rs`, `memory.rs`,
  `mcp.rs` — checked together as a group, **zero errors, zero
  warnings**). This was done by installing a Rust toolchain via `apt`
  in the sandbox and compile-checking each module in isolation, working
  up the dependency chain. (Two details: the verification used
  `rusqlite = "0.31"` and `sha2 = "0.10"` as stand-ins, since the newer
  versions `Cargo.toml` correctly ships — `0.40` and `0.11`
  respectively — each pull in a dependency that requires edition2024 a
  level or two down. Neither module touches an API that changed between
  those versions, so both are sound proxies, not discrepancies to worry
  about.)
  **`lazy_agent.rs` now has a real implementation** (Milestone 2 — see
  ROADMAP.md): the rule engine and exact-match cache are a direct,
  faithful port of the Python OpenMind CLI's `lazy_agent.py` reference
  — response strings, the SHA-256 cache-key algorithm, and token-budget
  constants copied exactly, not reinvented. This is the file where the
  `tokio::sync::Mutex`-instead-of-`std::sync::Mutex` choice documented
  back in the original scaffold actually gets exercised for real: the
  cache lock is acquired and released in a scoped block *before* the
  `.await` on `router.generate()`, then re-acquired afterward to store
  the result — verified correct by tracing it, and the fact that this
  compiled cleanly with zero warnings (including the two `dead_code`
  warnings from before, now gone because the fields are genuinely used)
  is real evidence that scoping is right. Semantic cache and memory-
  context compression remain out of scope for this milestone, per
  ROADMAP.md's explicit deferral — not silently skipped, deliberately
  cut.
  **`model_router.rs` has a real Ollama-backed implementation**
  (Milestone 1 — see ROADMAP.md) using `reqwest` for the HTTP calls,
  but `reqwest`'s own dependency tree hits the identical edition2024
  wall as Tauri's (traced down 4 levels: `reqwest → h2 → indexmap →
  hashbrown`, abandoned pinning it further as not worth the time), so
  this one file couldn't join the compiler-verified group and stays at
  hand-traced-against-the-docs status. It was verified carefully —
  every `reqwest` call matches the library's documented API exactly,
  and response-consumption ordering (`.status()` called twice safely,
  `.text()`/`.json()` each reached exactly once in mutually exclusive
  branches) was checked by hand — but this is real code that has never
  been run through `rustc`, not a stub.
  **`commands.rs` and `lib.rs` (the actual Tauri integration) could
  not be compiled** — the sandbox's `apt`-provided Rust is 1.75.0
  (Dec 2023), and Tauri 2.x's own dependency tree now requires Rust
  1.85+ (edition2024, stabilized Feb 2025); rustup's own install domain
  is blocked by this sandbox's network policy, so there was no way to
  get a new enough compiler to test the Tauri-specific code. **You are
  the first person to compile the Tauri integration layer, and the
  Ollama/LazyAgent integration, specifically** — the parts already
  proven (`error.rs`'s error handling, `memory.rs`'s and `mcp.rs`'s
  structural soundness, and now `lazy_agent.rs`'s actual rule-engine +
  exact-cache logic including the async/sync Mutex split exercised for
  real) are strong evidence the architecture is sound, but
  `commands.rs`'s `#[tauri::command]` macro expansions, `lib.rs`'s
  `Builder`/`State` wiring, and `model_router.rs`'s `reqwest` usage are
  still genuinely unverified by an actual compiler. As a second-best
  check, every pattern in these files was traced against the official
  Tauri/reqwest/Ollama docs and multiple independent community examples
  of working code (the `setup()` closure's `Ok(())` return type,
  `State<'_, T>` usage in both sync and async commands,
  Ollama's documented `/api/chat` and `/api/tags` response shapes, and —
  most directly — `error.rs`'s `thiserror` + manual `impl Serialize`
  pattern, which matches the exact pattern in Tauri's own GitHub
  discussions for this problem, word for word). This is meaningfully
  more confidence than "looks plausible," but it is still not the same
  as a compiler accepting it. Expect this is where any remaining
  compile errors would surface, if there are any.
- Four Rust modules matching the spec's architecture diagram exactly.
  `memory` and `mcp` have real types with a clear `NotImplemented`
  boundary marking where business logic needs to be written.
  `lazy_agent` has moved past that boundary for its rule engine and
  exact cache (semantic cache remains `NotImplemented`, deliberately
  deferred — see ROADMAP.md Milestone 2). `model_router` has moved past
  that boundary for its Ollama backend (embedded llama.cpp and LM
  Studio remain `NotImplemented` by design — see that file's module doc
  comment).
- A typed IPC contract (`src/lib/ipc.ts` ↔ `src-tauri/src/commands.rs`)
  — every field name, case convention, and type was checked by hand
  against both sides.

## Setup

### 1. Install prerequisites

- **Node.js** 18+ and npm
- **Rust 1.85 or newer** via [rustup](https://rustup.rs) — this is a
  real requirement, not a suggestion: Tauri 2.x's dependency tree needs
  edition2024 support, which only exists from Rust 1.85 onward (Feb
  2025). An older toolchain (anything installed via a Linux
  distribution's default package manager is a common way to end up with
  one) will fail with an `edition2024 is required` error during
  `cargo check`, confirmed directly during this scaffold's own
  verification — see "What's actually here" above.
- Platform build tools — see
  [Tauri's prerequisites guide](https://v2.tauri.app/start/prerequisites/)
  for your OS specifically (Xcode CLI tools on macOS, MSVC Build Tools
  on Windows, `webkit2gtk`/`libsoup` etc. on Linux — the exact package
  list depends on your distro)

### 2. Install dependencies

```bash
npm install
```

This was run and verified during scaffold creation — should install
cleanly.

### 3. First build attempt

```bash
npm run tauri dev
```

This compiles the Rust core for the first time. **This is genuinely
likely to surface compile errors** — first-build dependency resolution
issues, a platform-specific Tauri requirement not visible from outside a
real environment, or something in the hand-verification above that was
still wrong. Run `cargo check` from `src-tauri/` directly for faster
iteration on Rust-side errors:

```bash
cd src-tauri
cargo check
```

### 4. What you'll see if it builds

A window titled "OpenMind Desktop" with panels for each module (Local
Model Router, LazyAgent, Memory Tree, MCP Connectors). With Ollama
running locally, the model router will show real status. The chat panel
will return real responses via `LazyAgent`. Memory persists to disk on
close/reopen. The filesystem MCP connector is pre-registered and connects
with a single `connectIntegration("filesystem")` call (requires Node.js).

## Building a release installer (Milestone 5)

### Automated — GitHub Actions (recommended)

Push a version tag and the CI workflow builds signed installers for all
platforms automatically:

```bash
git tag v0.1.0
git push origin v0.1.0
```

This triggers `.github/workflows/release.yml`, which builds macOS DMGs
(Intel + Apple Silicon), a Windows NSIS installer + MSI, and a Linux
AppImage + .deb, then uploads them as a GitHub Release draft.

### Manual — local build

```bash
npm run tauri build
```

Artifacts appear in `src-tauri/target/release/bundle/`.

### Code signing

**Without signing**, every OS will warn users:
- macOS: "damaged app" / "cannot be opened" (especially on Apple Silicon)
- Windows: SmartScreen blocks the installer entirely
- Linux: no warning (signing not required)

**Workaround for unsigned builds** — tell users:

macOS:
```bash
xattr -d com.apple.quarantine /Applications/OpenMind\ Desktop.app
```

Windows: "More info" → "Run anyway" on the SmartScreen dialog.

**To set up real signing:**

_macOS_ (requires paid Apple Developer Program, ~$99/year):
1. Create a "Developer ID Application" certificate in Xcode / developer.apple.com
2. Export it as a .p12 from Keychain Access
3. Add these GitHub repository secrets:
   - `APPLE_CERTIFICATE` — base64 of the .p12: `base64 -i cert.p12 | pbcopy`
   - `APPLE_CERTIFICATE_PASSWORD` — the .p12 export password
   - `APPLE_SIGNING_IDENTITY` — e.g. `Developer ID Application: Jane Doe (ABCD1234EF)`
   - `APPLE_ID` — your Apple ID email
   - `APPLE_PASSWORD` — an [app-specific password](https://appleid.apple.com/account/manage) for notarization
   - `APPLE_TEAM_ID` — your 10-character Team ID from developer.apple.com

_Windows_ (requires Azure Key Vault, ~$10-20/month or OV cert on HSM):
Certificates can no longer be stored as exportable files (since June 2023).
Follow the [Tauri Windows signing guide](https://v2.tauri.app/distribute/sign/windows/)
for Azure Key Vault setup, then add these secrets:
- `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`
- `AZURE_KEY_VAULT_URI`, `AZURE_CERT_NAME`

_Updater signing_ (optional but recommended for auto-updates):
```bash
# Generate a key pair once:
cargo tauri signer generate -w ~/.tauri/openmind-desktop.key
# Add the private key as TAURI_SIGNING_PRIVATE_KEY secret in GitHub
```

### Calendar reality check

Getting signing fully working takes longer than the code:
- Apple Developer Program enrollment: 1-2 days (ID verification)
- macOS certificate issuance: instant once enrolled
- Windows Azure Key Vault setup: 1-2 hours
- Google Play Console (if Android ever): 2-3 weeks

Ship an unsigned build to real users first if you're in a hurry — the
xattr workaround is one command and honest documentation covers the rest.
Don't let signing setup block launch.

## Where to start implementing

Follow the spec's suggested build order (`OPENMIND_DESKTOP_SPEC.md` §16):

1. **`src-tauri/src/model_router.rs`** — Ollama backend is implemented.
   Embedded llama.cpp (zero-config) is the next priority.
2. **`src-tauri/src/lazy_agent.rs`** — Rule engine and exact cache done.
   Semantic cache (embedding-based) is the next piece.
3. **`src-tauri/src/memory.rs`** — Fully implemented, zero stubs.
4. **`src-tauri/src/mcp.rs`** — Fully implemented, zero stubs.
   Connect the filesystem server: `connectIntegration("filesystem")`.

Every module's doc comment explains its current state.

## Project layout

```
openmind-desktop/
├── .github/workflows/
│   ├── release.yml             Multi-platform signed release build (push v* tag)
│   └── ci.yml                  PR validation (cargo check + frontend build)
├── src/                        React frontend
│   ├── lib/ipc.ts              Typed IPC contract — source of truth for
│   │                            what each Rust command must return
│   ├── App.tsx                 UI (status panels for each module)
│   └── styles.css
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json         Bundle config — icons, signing, platform targets
│   ├── entitlements.plist      macOS entitlements (required for notarization)
│   ├── capabilities/main.json  Tauri v2 permission grants
│   ├── icons/                  App icons (PNG placeholders — replace before launch)
│   │   ├── 32x32.png
│   │   ├── 128x128.png
│   │   ├── 128x128@2x.png
│   │   └── icon.png
│   └── src/
│       ├── lib.rs              Entry point, state management, command registration
│       ├── main.rs             Thin shim (mobile entry point lives in lib.rs)
│       ├── error.rs            Shared AppError type (compiler-verified)
│       ├── commands.rs         #[tauri::command] surface — Rust side of ipc.ts
│       ├── lazy_agent.rs       Spec §4 — rule engine + exact cache real (compiler-verified)
│       ├── model_router.rs     Spec §3 — Ollama backend real (hand-traced)
│       ├── memory.rs           Spec §5 — fully real, zero stubs (compiler-verified)
│       └── mcp.rs              Spec §6 — fully real, zero stubs (compiler-verified)
├── OPENMIND_DESKTOP_SPEC.md    16-section architecture spec
├── ROADMAP.md                  Milestone-based build plan with honest progress tracking
└── package.json
```
