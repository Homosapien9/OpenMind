import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { load, type Store } from "@tauri-apps/plugin-store";
import {
  connectIntegration,
  disconnectIntegration,
  getAppSettings,
  getBackgroundLoopStatus,
  getConversationHistory,
  getModelStatus,
  getTokenSavings,
  listConnectors,
  listConversationThreads,
  sendChatMessageStreaming,
  updateAppSettings,
  type AppSettings,
  type BackgroundLoopStatus,
  type ConnectorManifest,
  type ConversationEntry,
  type ModelBackend,
  type ModelStatus,
  type ProviderSettings,
  type ThreadSummary,
  type TokenSavingsStats,
} from "./lib/ipc";

type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  pending?: boolean;
  meta?: string;
};

type DeltaEvent = { streamId: string; delta: string };
type FinishedEvent = { streamId: string };
type ErrorEvent = { streamId: string; message: string };

const DEFAULT_SETTINGS: AppSettings = {
  provider: {
    backend: "ollama",
    modelName: "",
    apiKey: "",
    baseUrl: "https://api.openai.com/v1",
    ollamaUrl: "http://127.0.0.1:11434",
    temperature: 0.3,
  },
  onboardingCompleted: false,
};

function backendLabel(backend: ModelBackend) {
  switch (backend) {
    case "open_ai":
      return "OpenAI";
    case "open_router":
      return "OpenRouter";
    case "anthropic":
      return "Anthropic";
    case "nvidia":
      return "NVIDIA";
    case "compatible":
      return "Custom compatible";
    default:
      return "Ollama";
  }
}

function uuid() {
  return crypto.randomUUID();
}

function defaultBaseUrlForBackend(backend: ModelBackend) {
  switch (backend) {
    case "open_router":
      return "https://openrouter.ai/api/v1";
    case "anthropic":
      return "https://api.anthropic.com";
    case "nvidia":
      return "https://integrate.api.nvidia.com/v1";
    case "compatible":
      return "";
    case "open_ai":
      return "https://api.openai.com/v1";
    default:
      return "https://api.openai.com/v1";
  }
}

function toChatMessages(history: ConversationEntry[]): ChatMessage[] {
  return history
    .filter((entry) => entry.role === "user" || entry.role === "assistant")
    .map((entry) => ({
      id: `${entry.timestamp}-${Math.random()}`,
      role: entry.role as "user" | "assistant",
      content: entry.content,
      meta: new Date(entry.timestamp).toLocaleString(),
    }));
}

export default function App() {
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [draftProvider, setDraftProvider] = useState<ProviderSettings>(DEFAULT_SETTINGS.provider);
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [tokenStats, setTokenStats] = useState<TokenSavingsStats | null>(null);
  const [memoryStatus, setMemoryStatus] = useState<BackgroundLoopStatus | null>(null);
  const [connectors, setConnectors] = useState<ConnectorManifest[]>([]);
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [activeThreadId, setActiveThreadId] = useState<string>(uuid());
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const storeRef = useRef<Store | null>(null);

  const onboarding = useMemo(() => {
    const providerConfigured =
      draftProvider.backend === "ollama"
        ? true
        : Boolean(draftProvider.apiKey?.trim() && draftProvider.modelName.trim());
    return [
      {
        label: "Provider configured",
        done: providerConfigured,
      },
      {
        label: "Model reachable",
        done: Boolean(status?.available),
      },
      {
        label: "Conversation memory active",
        done: threads.length > 0 || messages.length > 0,
      },
    ];
  }, [draftProvider, status, threads.length, messages.length]);

  async function refreshOverview() {
    const [nextStatus, nextTokens, nextMemory, nextConnectors, nextThreads] = await Promise.all([
      getModelStatus(),
      getTokenSavings(),
      getBackgroundLoopStatus(),
      listConnectors(),
      listConversationThreads(),
    ]);
    setStatus(nextStatus);
    setTokenStats(nextTokens);
    setMemoryStatus(nextMemory);
    setConnectors(nextConnectors);
    setThreads(nextThreads);
  }

  async function loadHistory(threadId: string) {
    if (!threadId) return;
    const history = await getConversationHistory(threadId);
    setMessages(toChatMessages(history));
  }

  useEffect(() => {
    let mounted = true;
    (async () => {
      try {
        const store = await load("ui.json", {
          defaults: {
            activeThreadId,
            draft: "",
          },
          autoSave: 100,
        });
        storeRef.current = store;
        const savedThreadId = (await store.get<string>("activeThreadId")) || activeThreadId;
        const savedDraft = (await store.get<string>("draft")) || "";
        const appSettings = await getAppSettings();
        if (!mounted) return;
        setSettings(appSettings);
        setDraftProvider(appSettings.provider);
        setActiveThreadId(savedThreadId);
        setInput(savedDraft);
        await Promise.all([refreshOverview(), loadHistory(savedThreadId)]);
      } catch (err) {
        if (!mounted) return;
        setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    if (!storeRef.current) return;
    void storeRef.current.set("draft", input);
  }, [input]);

  useEffect(() => {
    if (!storeRef.current) return;
    void storeRef.current.set("activeThreadId", activeThreadId);
  }, [activeThreadId]);

  async function handleSaveSettings() {
    setSavingSettings(true);
    setError(null);
    try {
      const saved = await updateAppSettings({
        provider: draftProvider,
        onboardingCompleted: true,
      });
      setSettings(saved);
      setDraftProvider(saved.provider);
      await refreshOverview();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingSettings(false);
    }
  }

  async function handleToggleConnector(connector: ConnectorManifest) {
    setError(null);
    try {
      if (connector.authState === "connected") {
        await disconnectIntegration(connector.id);
      } else {
        await connectIntegration(connector.id);
      }
      await refreshOverview();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleSelectThread(threadId: string) {
    setActiveThreadId(threadId);
    setError(null);
    try {
      await loadHistory(threadId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleNewThread() {
    const next = uuid();
    setActiveThreadId(next);
    setMessages([]);
    setInput("");
  }

  async function handleSend() {
    const message = input.trim();
    if (!message || busy) return;

    const threadId = activeThreadId || uuid();
    if (!activeThreadId) setActiveThreadId(threadId);
    const streamId = uuid();
    const assistantId = uuid();
    const userMessage: ChatMessage = { id: uuid(), role: "user", content: message };
    const placeholder: ChatMessage = { id: assistantId, role: "assistant", content: "", pending: true };

    setBusy(true);
    setError(null);
    setMessages((prev) => [...prev, userMessage, placeholder]);
    setInput("");

    const unlistenDelta = await listen<DeltaEvent>("chat-stream-delta", (event) => {
      if (event.payload.streamId !== streamId) return;
      setMessages((prev) =>
        prev.map((item) =>
          item.id === assistantId ? { ...item, content: item.content + event.payload.delta } : item,
        ),
      );
    });
    const unlistenFinished = await listen<FinishedEvent>("chat-stream-finished", () => undefined);
    const unlistenError = await listen<ErrorEvent>("chat-stream-error", (event) => {
      if (event.payload.streamId !== streamId) return;
      setError(event.payload.message);
    });

    try {
      const response = await sendChatMessageStreaming({ message, threadId }, streamId);
      setMessages((prev) =>
        prev.map((item) =>
          item.id === assistantId
            ? {
                ...item,
                content: response.text,
                pending: false,
                meta: `${response.source} · ${Math.round(response.latencyMs)}ms · ${response.tokensUsed} tokens`,
              }
            : item,
        ),
      );
      await refreshOverview();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setMessages((prev) =>
        prev.map((item) =>
          item.id === assistantId ? { ...item, content: `Error: ${msg}`, pending: false } : item,
        ),
      );
    } finally {
      unlistenDelta();
      unlistenFinished();
      unlistenError();
      setBusy(false);
    }
  }

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand-card">
          <div className="eyebrow">OpenMind Desktop</div>
          <h1>From scaffold to real assistant</h1>
          <p>
            Multi-provider model routing, streaming chat, saved threads, memory recall, and MCP tool wiring.
          </p>
          <button className="primary-button ghost" onClick={() => void handleNewThread()}>
            New thread
          </button>
        </div>

        <section className="card">
          <div className="section-title">Provider</div>
          <label>
            Backend
            <select
              value={draftProvider.backend}
              onChange={(e) =>
                setDraftProvider((prev) => {
                  const backend = e.target.value as ModelBackend;
                  return {
                    ...prev,
                    backend,
                    baseUrl: defaultBaseUrlForBackend(backend),
                  };
                })
              }
            >
              <option value="ollama">Ollama</option>
              <option value="open_ai">OpenAI</option>
              <option value="open_router">OpenRouter</option>
              <option value="anthropic">Anthropic</option>
              <option value="nvidia">NVIDIA</option>
              <option value="compatible">Custom OpenAI-compatible</option>
            </select>
          </label>
          <label>
            Model
            <input
              value={draftProvider.modelName}
              onChange={(e) => setDraftProvider((prev) => ({ ...prev, modelName: e.target.value }))}
              placeholder={draftProvider.backend === "ollama" ? "e.g. llama3.1:8b" : "e.g. gpt-4.1-mini"}
            />
          </label>
          {draftProvider.backend === "ollama" ? (
            <label>
              Ollama URL
              <input
                value={draftProvider.ollamaUrl ?? ""}
                onChange={(e) => setDraftProvider((prev) => ({ ...prev, ollamaUrl: e.target.value }))}
                placeholder="http://127.0.0.1:11434"
              />
            </label>
          ) : (
            <>
              <label>
                API key
                <input
                  type="password"
                  value={draftProvider.apiKey ?? ""}
                  onChange={(e) => setDraftProvider((prev) => ({ ...prev, apiKey: e.target.value }))}
                  placeholder="Paste API key"
                />
              </label>
              <label>
                Base URL
                <input
                  value={draftProvider.baseUrl ?? ""}
                  onChange={(e) => setDraftProvider((prev) => ({ ...prev, baseUrl: e.target.value }))}
                  placeholder="Provider API base URL"
                />
              </label>
            </>
          )}
          <label>
            Temperature
            <input
              type="number"
              min={0}
              max={2}
              step={0.1}
              value={draftProvider.temperature ?? 0.3}
              onChange={(e) =>
                setDraftProvider((prev) => ({ ...prev, temperature: Number(e.target.value) }))
              }
            />
          </label>
          <button className="primary-button" disabled={savingSettings} onClick={() => void handleSaveSettings()}>
            {savingSettings ? "Saving…" : "Save settings"}
          </button>
          {status && (
            <div className={`status-pill ${status.available ? "success" : "warning"}`}>
              {backendLabel(status.backend)} · {status.available ? "ready" : "needs attention"}
            </div>
          )}
          {status?.detail && <p className="muted small">{status.detail}</p>}
        </section>

        <section className="card">
          <div className="section-title">Onboarding</div>
          <div className="checklist">
            {onboarding.map((item) => (
              <div key={item.label} className={`check-item ${item.done ? "done" : "todo"}`}>
                <span>{item.done ? "●" : "○"}</span>
                <span>{item.label}</span>
              </div>
            ))}
          </div>
        </section>

        <section className="card">
          <div className="section-title">Threads</div>
          <div className="thread-list">
            {threads.length === 0 && <div className="empty-state">No saved threads yet.</div>}
            {threads.map((thread) => (
              <button
                key={thread.threadId}
                className={`thread-row ${thread.threadId === activeThreadId ? "active" : ""}`}
                onClick={() => void handleSelectThread(thread.threadId)}
              >
                <strong>{thread.preview || "New conversation"}</strong>
                <span>
                  {thread.messageCount} msgs · {new Date(thread.lastUpdatedAt).toLocaleString()}
                </span>
              </button>
            ))}
          </div>
        </section>
      </aside>

      <main className="main-panel">
        <section className="hero-row">
          <div className="hero-card">
            <div className="eyebrow">Active backend</div>
            <h2>{status ? backendLabel(status.backend) : backendLabel(settings.provider.backend)}</h2>
            <p>
              {status?.modelName || draftProvider.modelName || "Choose a local or remote model and start chatting."}
            </p>
          </div>
          <div className="metric-card">
            <span>Token savings</span>
            <strong>{tokenStats ? `${tokenStats.savingsPct.toFixed(1)}%` : "—"}</strong>
          </div>
          <div className="metric-card">
            <span>Memory loop</span>
            <strong>{memoryStatus?.running ? "running" : "idle"}</strong>
          </div>
        </section>

        <section className="chat-card">
          <div className="chat-header">
            <div>
              <div className="section-title">Chat</div>
              <p className="muted">
                Streaming responses, thread persistence, and memory/tool augmentation are enabled.
              </p>
            </div>
            <div className="status-pill neutral">thread {activeThreadId.slice(0, 8)}</div>
          </div>

          <div className="messages">
            {messages.length === 0 && (
              <div className="empty-chat">
                Ask a question, plan a task, or use a connected MCP tool via normal conversation.
              </div>
            )}
            {messages.map((message) => (
              <article key={message.id} className={`bubble ${message.role}`}>
                <div className="bubble-role">{message.role === "user" ? "You" : "OpenMind"}</div>
                <div className="bubble-content">{message.content || (message.pending ? "Thinking…" : "")}</div>
                {message.meta && <div className="bubble-meta">{message.meta}</div>}
              </article>
            ))}
          </div>

          <div className="composer">
            <textarea
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="Ask anything — the app will reuse thread history, search local memory, and call connected tools when needed."
              rows={4}
            />
            <div className="composer-actions">
              <span className="muted small">Shift+Enter for newline</span>
              <button className="primary-button" disabled={busy || !input.trim()} onClick={() => void handleSend()}>
                {busy ? "Streaming…" : "Send"}
              </button>
            </div>
          </div>
        </section>

        <section className="bottom-grid">
          <div className="card">
            <div className="section-title">Connectors</div>
            <div className="connector-list">
              {connectors.length === 0 && <div className="empty-state">No connectors registered.</div>}
              {connectors.map((connector) => (
                <div key={connector.id} className="connector-row">
                  <div>
                    <strong>{connector.name}</strong>
                    <div className="muted small">{connector.transport}</div>
                  </div>
                  <button className="secondary-button" onClick={() => void handleToggleConnector(connector)}>
                    {connector.authState === "connected" ? "Disconnect" : "Connect"}
                  </button>
                </div>
              ))}
            </div>
          </div>

          <div className="card">
            <div className="section-title">System notes</div>
            <ul className="notes-list">
              <li>Provider settings are persisted.</li>
              <li>Chat threads are saved in SQLite.</li>
              <li>OAuth loopback now uses PKCE and timeout protection.</li>
              <li>MCP disconnect shuts subprocesses down more cleanly.</li>
            </ul>
          </div>
        </section>

        {error && <div className="error-banner">{error}</div>}
      </main>
    </div>
  );
}
