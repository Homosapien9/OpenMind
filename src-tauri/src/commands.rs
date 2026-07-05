//! Tauri command surface.
//!
//! Every function here is the Rust side of a call in `src/lib/ipc.ts`.
//! Names, parameter shapes, and return types are kept in lockstep with
//! that file deliberately — this is the typed contract described in
//! ipc.ts's module doc comment. When a module (lazy_agent, model_router,
//! memory, mcp) gets a real implementation, the command below it starts
//! returning real data with zero change to the frontend.

use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use crate::error::AppResult;
use crate::lazy_agent::{LazyAgent, LazyResponse, TokenSavingsStats};
use crate::mcp::{CallToolResult, ConnectorManifest, ConnectorRegistry, McpTool};
use crate::memory::{BackgroundLoopStatus, MemoryNode, MemoryTree};
use crate::model_router::{ModelRouter, ModelStatus};
use crate::oauth::{self, OAuthConfig, OAuthToken};

// ── Request types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub message: String,
    /// Context grouping for future multi-turn conversation support.
    /// Received from the frontend but not yet threaded through to
    /// LazyAgent — wired when conversation history is implemented.
    #[allow(dead_code)]
    pub thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMemoryRequest {
    pub content: String,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
}

// ── Local Model Router (spec §3) ─────────────────────────────────────────

#[tauri::command]
pub async fn get_model_status(router: State<'_, ModelRouter>) -> AppResult<ModelStatus> {
    router.status().await
}

// ── LazyAgent (spec §4) ──────────────────────────────────────────────────

#[tauri::command]
pub async fn send_chat_message(
    req: ChatRequest,
    agent: State<'_, LazyAgent>,
    router: State<'_, ModelRouter>,
) -> AppResult<LazyResponse> {
    agent.ask(&req.message, "query", &*router).await
}

#[tauri::command]
pub async fn get_token_savings(agent: State<'_, LazyAgent>) -> AppResult<TokenSavingsStats> {
    agent.stats().await
}

// ── Memory Tree (spec §5) ────────────────────────────────────────────────

#[tauri::command]
pub fn add_memory(req: AddMemoryRequest, memory: State<'_, MemoryTree>) -> AppResult<MemoryNode> {
    memory.add_memory(
        &req.content,
        req.category.as_deref().unwrap_or("general"),
        req.source.as_deref().unwrap_or(""),
        &req.tags.unwrap_or_default(),
    )
}

#[tauri::command]
pub fn list_memory_tree(memory: State<'_, MemoryTree>) -> AppResult<Vec<MemoryNode>> {
    memory.list_tree()
}

#[tauri::command]
pub fn search_memory(query: String, memory: State<'_, MemoryTree>) -> AppResult<Vec<MemoryNode>> {
    memory.search(&query)
}

#[tauri::command]
pub fn get_background_loop_status(memory: State<'_, MemoryTree>) -> AppResult<BackgroundLoopStatus> {
    memory.background_loop_status()
}

// ── MCP Integration Framework (spec §6) ──────────────────────────────────

#[tauri::command]
pub async fn list_connectors(
    registry: State<'_, ConnectorRegistry>,
) -> AppResult<Vec<ConnectorManifest>> {
    registry.list().await
}

#[tauri::command]
pub async fn connect_integration(
    connector_id: String,
    registry: State<'_, ConnectorRegistry>,
) -> AppResult<ConnectorManifest> {
    registry.connect(&connector_id).await
}

#[tauri::command]
pub async fn disconnect_integration(
    connector_id: String,
    registry: State<'_, ConnectorRegistry>,
) -> AppResult<()> {
    registry.disconnect(&connector_id).await
}

#[tauri::command]
pub async fn list_tools(
    connector_id: String,
    registry: State<'_, ConnectorRegistry>,
) -> AppResult<Vec<McpTool>> {
    registry.list_tools(&connector_id).await
}

#[tauri::command]
pub async fn call_tool(
    connector_id: String,
    tool_name: String,
    arguments: Option<Value>,
    registry: State<'_, ConnectorRegistry>,
) -> AppResult<CallToolResult> {
    registry.call_tool(&connector_id, &tool_name, arguments).await
}

// ── OAuth (spec §6, Milestone 6) ──────────────────────────────────────────

/// Begin the Google OAuth loopback flow for a connector.
/// Opens the system browser, waits for the redirect callback,
/// exchanges the code for tokens, stores them in the OS keychain.
///
/// Blocks until the browser interaction completes (typically 10-60 sec).
/// The frontend should show a loading/waiting state while this runs.
#[tauri::command]
pub async fn begin_oauth(
    connector_id: String,
    config: OAuthConfig,
    router: State<'_, ModelRouter>,
) -> AppResult<OAuthToken> {
    // Reuse ModelRouter's reqwest::Client rather than creating a second
    // HTTP client — shares the connection pool.
    oauth::run_loopback_flow(&config, &connector_id, &router.http_client()).await
}

/// Check whether a stored token exists in the OS keychain for a connector.
/// Does NOT validate the token against Google's servers.
#[tauri::command]
pub fn get_oauth_token(connector_id: String) -> AppResult<Option<OAuthToken>> {
    oauth::load_token(&connector_id)
}

/// Remove a stored token from the OS keychain (used on disconnect).
#[tauri::command]
pub fn revoke_oauth_token(connector_id: String) -> AppResult<()> {
    oauth::delete_token(&connector_id)
}
