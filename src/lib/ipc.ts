import { invoke } from "@tauri-apps/api/core";

export type ModelBackend =
  | "ollama"
  | "open_ai"
  | "open_router"
  | "anthropic"
  | "nvidia"
  | "compatible";

export interface ModelStatus {
  backend: ModelBackend;
  modelName: string;
  available: boolean;
  detail?: string | null;
}

export interface ProviderSettings {
  backend: ModelBackend;
  modelName: string;
  apiKey?: string | null;
  baseUrl?: string | null;
  ollamaUrl?: string | null;
  temperature?: number | null;
}

export interface AppSettings {
  provider: ProviderSettings;
  onboardingCompleted: boolean;
}

export interface LazyResponse {
  text: string;
  source: "rule" | "exact_cache" | "semantic_cache" | "llm";
  tokensUsed: number;
  tokensSaved: number;
  latencyMs: number;
}

export interface ChatRequest {
  message: string;
  threadId?: string;
}

export interface TokenSavingsStats {
  totalCalls: number;
  cacheHits: number;
  tokensUsed: number;
  tokensSaved: number;
  savingsPct: number;
}

export interface MemoryNode {
  id: string;
  title: string;
  content: string;
  source: string;
  createdAt: string;
  updatedAt: string;
  children: string[];
}

export interface AddMemoryRequest {
  content: string;
  category?: string;
  source?: string;
  tags?: string[];
}

export interface BackgroundLoopStatus {
  running: boolean;
  lastRunAt: string | null;
  intervalMinutes: number;
  nextRunAt: string | null;
}

export interface ConversationEntry {
  threadId: string;
  role: string;
  content: string;
  timestamp: string;
  metadata: string;
}

export interface ThreadSummary {
  threadId: string;
  lastUpdatedAt: string;
  preview: string;
  messageCount: number;
}

export type ConnectorAuthState = "disconnected" | "connecting" | "connected" | "error";

export interface ConnectorManifest {
  id: string;
  name: string;
  transport: "stdio" | "streamable_http";
  authState: ConnectorAuthState;
  fetchIntervalMinutes: number;
  lastFetchAt: string | null;
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

export async function getModelStatus(): Promise<ModelStatus> {
  return invoke("get_model_status");
}

export async function getAppSettings(): Promise<AppSettings> {
  return invoke("get_app_settings");
}

export async function updateAppSettings(newSettings: AppSettings): Promise<AppSettings> {
  return invoke("update_app_settings", { newSettings });
}

export async function sendChatMessage(req: ChatRequest): Promise<LazyResponse> {
  return invoke("send_chat_message", { req });
}

export async function sendChatMessageStreaming(
  req: ChatRequest,
  streamId: string,
): Promise<LazyResponse> {
  return invoke("send_chat_message_streaming", { req, streamId });
}

export async function getTokenSavings(): Promise<TokenSavingsStats> {
  return invoke("get_token_savings");
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

export async function getBackgroundLoopStatus(): Promise<BackgroundLoopStatus> {
  return invoke("get_background_loop_status");
}

export async function getConversationHistory(threadId: string): Promise<ConversationEntry[]> {
  return invoke("get_conversation_history", { threadId });
}

export async function listConversationThreads(): Promise<ThreadSummary[]> {
  return invoke("list_conversation_threads");
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

export async function beginOAuth(
  connectorId: string,
  config: OAuthConfig,
): Promise<OAuthToken> {
  return invoke("begin_oauth", { connectorId, config });
}

export async function getOAuthToken(connectorId: string): Promise<OAuthToken | null> {
  return invoke("get_oauth_token", { connectorId });
}

export async function revokeOAuthToken(connectorId: string): Promise<void> {
  return invoke("revoke_oauth_token", { connectorId });
}
