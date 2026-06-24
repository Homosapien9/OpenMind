/**
 * IPC bindings — the typed contract between the React frontend and the
 * Rust core, invoked over Tauri's `invoke()` bridge.
 *
 * STATUS: scaffold only. Every function below calls a `#[tauri::command]`
 * that exists in src-tauri/src/commands.rs but currently returns
 * `Err("not implemented")`. The types are real and match the spec's
 * architecture (§7) — the implementations are not.
 *
 * As each Rust module (lazy_agent, model_router, memory, mcp) gets a real
 * implementation, the corresponding command here starts returning real
 * data with no change needed on the frontend side — that's the point of
 * defining the contract first.
 */

import { invoke } from "@tauri-apps/api/core";

// ── Local Model Router (spec §3) ────────────────────────────────────────

export type ModelBackend = "embedded" | "ollama" | "lm_studio";

export interface ModelStatus {
  backend: ModelBackend;
  modelName: string;
  available: boolean;
}

export async function getModelStatus(): Promise<ModelStatus> {
  return invoke("get_model_status");
}

// ── LazyAgent (spec §4) ──────────────────────────────────────────────────

export type LazySource = "rule" | "exact_cache" | "semantic_cache" | "llm";

export interface LazyResponse {
  text: string;
  source: LazySource;
  tokensUsed: number;
  tokensSaved: number;
  latencyMs: number;
}

export interface ChatRequest {
  message: string;
  /** Optional conversation/thread id for context grouping. */
  threadId?: string;
}

export async function sendChatMessage(req: ChatRequest): Promise<LazyResponse> {
  return invoke("send_chat_message", { req });
}

export interface TokenSavingsStats {
  totalCalls: number;
  cacheHits: number;
  tokensUsed: number;
  tokensSaved: number;
  savingsPct: number;
}

export async function getTokenSavings(): Promise<TokenSavingsStats> {
  return invoke("get_token_savings");
}

// ── Memory Tree (spec §5) ────────────────────────────────────────────────

export interface MemoryNode {
  id: string;
  title: string;
  /** Markdown content, Obsidian-vault-compatible. */
  content: string;
  source: string;
  createdAt: string;
  updatedAt: string;
  children: string[]; // child node ids
}

export interface AddMemoryRequest {
  content: string;
  category?: string;
  source?: string;
  tags?: string[];
}

export async function addMemory(req: AddMemoryRequest): Promise<MemoryNode> {
  return invoke("add_memory", { req });
}

export async function listMemoryTree(): Promise<MemoryNode[]> {
  return invoke("list_memory_tree");
}

export async function searchMemory(query: string): Promise<MemoryNode[]> {
  return invoke("search_memory", { query });
}

export interface BackgroundLoopStatus {
  running: boolean;
  lastRunAt: string | null;
  intervalMinutes: number;
  nextRunAt: string | null;
}

export async function getBackgroundLoopStatus(): Promise<BackgroundLoopStatus> {
  return invoke("get_background_loop_status");
}

// ── MCP Integration Framework (spec §6) ──────────────────────────────────

export type ConnectorAuthState = "disconnected" | "connecting" | "connected" | "error";

export interface ConnectorManifest {
  id: string;
  name: string;
  /** e.g. "stdio" | "streamable_http" — MCP transport this connector uses */
  transport: "stdio" | "streamable_http";
  authState: ConnectorAuthState;
  fetchIntervalMinutes: number;
  lastFetchAt: string | null;
}

export async function listConnectors(): Promise<ConnectorManifest[]> {
  return invoke("list_connectors");
}

export async function connectIntegration(connectorId: string): Promise<ConnectorManifest> {
  return invoke("connect_integration", { connectorId });
}

export async function disconnectIntegration(connectorId: string): Promise<void> {
  return invoke("disconnect_integration", { connectorId });
}

export interface McpTool {
  name: string;
  description?: string;
}

export interface ToolContent {
  type: string;
  text?: string;
}

export interface CallToolResult {
  content: ToolContent[];
  isError: boolean;
}

export async function listTools(connectorId: string): Promise<McpTool[]> {
  return invoke("list_tools", { connectorId });
}

export async function callTool(
  connectorId: string,
  toolName: string,
  args?: Record<string, unknown>,
): Promise<CallToolResult> {
  return invoke("call_tool", { connectorId, toolName, arguments: args });
}

// ── OAuth (spec §6, Milestone 6) ────────────────────────────────────────

export interface OAuthConfig {
  clientId: string;
  clientSecret: string;
  scopes: string[];
}

export interface OAuthToken {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number;
  scope: string;
  tokenType: string;
}

/** Gmail read-only scope — the first connector for Milestone 6. */
export const GMAIL_READONLY_SCOPE =
  "https://www.googleapis.com/auth/gmail.readonly";

/**
 * Begin the Google OAuth loopback flow.
 * Opens the system browser. Resolves when the user completes consent
 * (or rejects on denial/error). Shows a loading state in the UI while
 * waiting — this can take 10-60 seconds depending on the user.
 */
export async function beginOAuth(
  connectorId: string,
  config: OAuthConfig,
): Promise<OAuthToken> {
  return invoke("begin_oauth", { connectorId, config });
}

/**
 * Check if a stored token exists for a connector.
 * Returns null if no token is stored (not yet connected).
 */
export async function getOAuthToken(
  connectorId: string,
): Promise<OAuthToken | null> {
  return invoke("get_oauth_token", { connectorId });
}

/**
 * Remove a stored token from the OS keychain.
 * Used when the user disconnects a connector.
 */
export async function revokeOAuthToken(connectorId: string): Promise<void> {
  return invoke("revoke_oauth_token", { connectorId });
}
