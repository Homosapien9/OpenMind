use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::lazy_agent::{LazyAgent, LazyResponse, StreamTarget, TokenSavingsStats};
use crate::mcp::{CallToolResult, ConnectorManifest, ConnectorRegistry, McpTool};
use crate::memory::{BackgroundLoopStatus, ConversationEntry, MemoryNode, MemoryTree, ThreadSummary};
use crate::model_router::{ModelRouter, ModelStatus};
use crate::oauth::{self, OAuthConfig, OAuthToken};
use crate::settings::{AppSettings, SettingsStore};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub message: String,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamFinishedPayload<'a> {
    stream_id: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamErrorPayload<'a> {
    stream_id: &'a str,
    message: &'a str,
}

#[tauri::command]
pub async fn get_model_status(
    router: State<'_, ModelRouter>,
    settings: State<'_, SettingsStore>,
) -> AppResult<ModelStatus> {
    let cfg = settings.get()?.provider;
    router.status(&cfg).await
}

#[tauri::command]
pub fn get_app_settings(settings: State<'_, SettingsStore>) -> AppResult<AppSettings> {
    settings.get()
}

#[tauri::command]
pub fn update_app_settings(
    new_settings: AppSettings,
    settings: State<'_, SettingsStore>,
) -> AppResult<AppSettings> {
    settings.update(new_settings)
}

#[tauri::command]
pub async fn send_chat_message(
    req: ChatRequest,
    agent: State<'_, LazyAgent>,
    router: State<'_, ModelRouter>,
    memory: State<'_, MemoryTree>,
    registry: State<'_, ConnectorRegistry>,
    settings: State<'_, SettingsStore>,
) -> AppResult<LazyResponse> {
    let provider = settings.get()?.provider;
    agent
        .ask(
            &req.message,
            "query",
            req.thread_id.as_deref(),
            &router,
            &memory,
            &registry,
            &provider,
            None,
        )
        .await
}

#[tauri::command]
pub async fn send_chat_message_streaming(
    req: ChatRequest,
    stream_id: String,
    app: AppHandle,
    agent: State<'_, LazyAgent>,
    router: State<'_, ModelRouter>,
    memory: State<'_, MemoryTree>,
    registry: State<'_, ConnectorRegistry>,
    settings: State<'_, SettingsStore>,
) -> AppResult<LazyResponse> {
    let provider = settings.get()?.provider;
    let stream_target = StreamTarget { stream_id: stream_id.clone(), app: app.clone() };

    let result = agent
        .ask(
            &req.message,
            "query",
            req.thread_id.as_deref(),
            &router,
            &memory,
            &registry,
            &provider,
            Some(&stream_target),
        )
        .await;

    match result {
        Ok(resp) => {
            use tauri::Emitter;
            app.emit(
                "chat-stream-finished",
                StreamFinishedPayload {
                    stream_id: &stream_id,
                },
            )
            .map_err(|e| AppError::Other(anyhow::anyhow!("stream finish emit failed: {e}")))?;
            Ok(resp)
        }
        Err(err) => {
            use tauri::Emitter;
            let message = err.to_string();
            let _ = app.emit(
                "chat-stream-error",
                StreamErrorPayload {
                    stream_id: &stream_id,
                    message: &message,
                },
            );
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn get_token_savings(agent: State<'_, LazyAgent>) -> AppResult<TokenSavingsStats> {
    agent.stats().await
}

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

#[tauri::command]
pub fn get_conversation_history(
    thread_id: String,
    memory: State<'_, MemoryTree>,
) -> AppResult<Vec<ConversationEntry>> {
    memory.conversation_history(&thread_id, 200)
}

#[tauri::command]
pub fn list_conversation_threads(memory: State<'_, MemoryTree>) -> AppResult<Vec<ThreadSummary>> {
    memory.list_threads(50)
}

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

#[tauri::command]
pub async fn begin_oauth(
    connector_id: String,
    config: OAuthConfig,
    router: State<'_, ModelRouter>,
) -> AppResult<OAuthToken> {
    oauth::run_loopback_flow(&config, &connector_id, &router.http_client()).await
}

#[tauri::command]
pub fn get_oauth_token(connector_id: String) -> AppResult<Option<OAuthToken>> {
    oauth::load_token(&connector_id)
}

#[tauri::command]
pub fn revoke_oauth_token(connector_id: String) -> AppResult<()> {
    oauth::delete_token(&connector_id)
}
