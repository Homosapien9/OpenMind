import { useEffect, useState } from "react";
import {
  getModelStatus,
  getTokenSavings,
  listConnectors,
  getBackgroundLoopStatus,
  type ModelStatus,
  type TokenSavingsStats,
  type ConnectorManifest,
  type BackgroundLoopStatus,
} from "./lib/ipc";

type PanelState<T> =
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; message: string };

function usePanel<T>(fetcher: () => Promise<T>): PanelState<T> {
  const [state, setState] = useState<PanelState<T>>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    fetcher()
      .then((data) => {
        if (!cancelled) setState({ status: "ok", data });
      })
      .catch((err) => {
        if (!cancelled) {
          setState({
            status: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return state;
}

function StatusBadge({ state }: { state: PanelState<unknown> }) {
  if (state.status === "loading") return <span className="badge badge-loading">loading…</span>;
  if (state.status === "error") return <span className="badge badge-error">not wired yet</span>;
  return <span className="badge badge-ok">ok</span>;
}

function ModelRouterPanel() {
  const state = usePanel<ModelStatus>(getModelStatus);
  return (
    <section className="panel">
      <header>
        <h2>Local Model Router</h2>
        <StatusBadge state={state} />
      </header>
      <p className="panel-desc">
        Embedded llama.cpp / Ollama / LM Studio — zero cloud fallback, by design.
      </p>
      {state.status === "ok" && (
        <dl>
          <dt>Backend</dt>
          <dd>{state.data.backend}</dd>
          <dt>Model</dt>
          <dd>{state.data.modelName}</dd>
          <dt>Available</dt>
          <dd>{state.data.available ? "yes" : "no"}</dd>
        </dl>
      )}
      {state.status === "error" && <p className="stub-note">{state.message}</p>}
    </section>
  );
}

function LazyAgentPanel() {
  const state = usePanel<TokenSavingsStats>(getTokenSavings);
  return (
    <section className="panel">
      <header>
        <h2>LazyAgent</h2>
        <StatusBadge state={state} />
      </header>
      <p className="panel-desc">Gate → cache → compress → act. Target: ~95% token interception.</p>
      {state.status === "ok" && (
        <dl>
          <dt>Calls</dt>
          <dd>{state.data.totalCalls}</dd>
          <dt>Cache hits</dt>
          <dd>{state.data.cacheHits}</dd>
          <dt>Savings</dt>
          <dd>{state.data.savingsPct.toFixed(1)}%</dd>
        </dl>
      )}
      {state.status === "error" && <p className="stub-note">{state.message}</p>}
    </section>
  );
}

function MemoryTreePanel() {
  const state = usePanel<BackgroundLoopStatus>(getBackgroundLoopStatus);
  return (
    <section className="panel">
      <header>
        <h2>Memory Tree</h2>
        <StatusBadge state={state} />
      </header>
      <p className="panel-desc">
        Local SQLite + Obsidian-compatible Markdown vault. Background loop re-indexes on a
        schedule.
      </p>
      {state.status === "ok" && (
        <dl>
          <dt>Running</dt>
          <dd>{state.data.running ? "yes" : "no"}</dd>
          <dt>Interval</dt>
          <dd>{state.data.intervalMinutes} min</dd>
          <dt>Last run</dt>
          <dd>{state.data.lastRunAt ?? "never"}</dd>
        </dl>
      )}
      {state.status === "error" && <p className="stub-note">{state.message}</p>}
    </section>
  );
}

function ConnectorsPanel() {
  const state = usePanel<ConnectorManifest[]>(listConnectors);
  return (
    <section className="panel">
      <header>
        <h2>MCP Connectors</h2>
        <StatusBadge state={state} />
      </header>
      <p className="panel-desc">
        MCP-native integration framework. OAuth handled locally via loopback redirect — never
        proxied.
      </p>
      {state.status === "ok" && (
        <ul className="connector-list">
          {state.data.length === 0 && <li className="stub-note">No connectors registered yet.</li>}
          {state.data.map((c) => (
            <li key={c.id}>
              {c.name} — <span className={`auth-${c.authState}`}>{c.authState}</span>
            </li>
          ))}
        </ul>
      )}
      {state.status === "error" && <p className="stub-note">{state.message}</p>}
    </section>
  );
}

export default function App() {
  return (
    <div className="app">
      <header className="app-header">
        <h1>🧠 OpenMind Desktop</h1>
        <p className="tagline">
          Fully local-first. Every claim, literally true. <strong>v0.1.0 — scaffold</strong>
        </p>
      </header>

      <div className="scaffold-notice">
        This is an architecture scaffold, not a working app yet. Every panel below calls a real
        Tauri IPC command — the commands currently return <code>not implemented</code> until each
        Rust module is built out. See <code>src-tauri/src/commands.rs</code>.
      </div>

      <main className="panel-grid">
        <ModelRouterPanel />
        <LazyAgentPanel />
        <MemoryTreePanel />
        <ConnectorsPanel />
      </main>
    </div>
  );
}
