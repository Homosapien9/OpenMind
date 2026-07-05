use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    #[serde(rename = "streamable_http")]
    StreamableHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAuthState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorManifest {
    pub id: String,
    pub name: String,
    pub transport: Transport,
    pub auth_state: ConnectorAuthState,
    pub fetch_interval_minutes: u32,
    pub last_fetch_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptToolInfo {
    pub connector_id: String,
    pub connector_name: String,
    pub tool_name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: &'static str,
    capabilities: ClientCapabilities,
    client_info: ClientInfo,
}

#[derive(Debug, Serialize)]
struct ClientCapabilities {
    roots: RootsCapability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RootsCapability {
    list_changed: bool,
}

#[derive(Debug, Serialize)]
struct ClientInfo {
    name: &'static str,
    version: &'static str,
}

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

struct McpConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpConnection {
    async fn request(&mut self, method: &str, params: Option<Value>) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&req)
            .map_err(|e| AppError::Connector(format!("serialize request: {e}")))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AppError::Connector(format!("write to server stdin: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| AppError::Connector(format!("flush server stdin: {e}")))?;

        loop {
            let mut buf = String::new();
            let n = self
                .stdout
                .read_line(&mut buf)
                .await
                .map_err(|e| AppError::Connector(format!("read from server stdout: {e}")))?;
            if n == 0 {
                return Err(AppError::Connector(
                    "MCP server closed stdout unexpectedly".to_string(),
                ));
            }
            let buf = buf.trim();
            if buf.is_empty() {
                continue;
            }

            let resp: JsonRpcResponse = match serde_json::from_str(buf) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let matches = resp
                .id
                .as_ref()
                .map(|v| match v {
                    Value::Number(n) => n.as_u64() == Some(id),
                    _ => false,
                })
                .unwrap_or(false);

            if !matches {
                continue;
            }

            if let Some(err) = resp.error {
                return Err(AppError::Connector(format!(
                    "MCP error {}: {}",
                    err.code, err.message
                )));
            }

            return resp.result.ok_or_else(|| {
                AppError::Connector("MCP response had neither result nor error".to_string())
            });
        }
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> AppResult<()> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&notif)
            .map_err(|e| AppError::Connector(format!("serialize notification: {e}")))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AppError::Connector(format!("write notification: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| AppError::Connector(format!("flush notification: {e}")))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ConnectorDescriptor {
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
}

pub struct ConnectorRegistry {
    descriptors: Vec<ConnectorDescriptor>,
    connections: Mutex<HashMap<String, McpConnection>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self {
            descriptors: vec![ConnectorDescriptor {
                id: "filesystem".to_string(),
                name: "Filesystem".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-filesystem".to_string(),
                    std::env::temp_dir().to_string_lossy().into_owned(),
                ],
            }],
            connections: Mutex::new(HashMap::new()),
        }
    }

    pub async fn list(&self) -> AppResult<Vec<ConnectorManifest>> {
        let conns = self.connections.lock().await;
        Ok(self
            .descriptors
            .iter()
            .map(|d| ConnectorManifest {
                id: d.id.clone(),
                name: d.name.clone(),
                transport: Transport::Stdio,
                auth_state: if conns.contains_key(&d.id) {
                    ConnectorAuthState::Connected
                } else {
                    ConnectorAuthState::Disconnected
                },
                fetch_interval_minutes: 0,
                last_fetch_at: None,
            })
            .collect())
    }

    pub async fn connect(&self, connector_id: &str) -> AppResult<ConnectorManifest> {
        let descriptor = self
            .descriptors
            .iter()
            .find(|d| d.id == connector_id)
            .ok_or_else(|| AppError::Connector(format!("unknown connector: {connector_id}")))?;

        let mut conns = self.connections.lock().await;
        if conns.contains_key(connector_id) {
            return Ok(ConnectorManifest {
                id: descriptor.id.clone(),
                name: descriptor.name.clone(),
                transport: Transport::Stdio,
                auth_state: ConnectorAuthState::Connected,
                fetch_interval_minutes: 0,
                last_fetch_at: None,
            });
        }

        let mut cmd_iter = descriptor.command.iter();
        let program = cmd_iter
            .next()
            .ok_or_else(|| AppError::Connector("connector command is empty".to_string()))?;

        let mut child = Command::new(program)
            .args(cmd_iter)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AppError::Connector(format!("failed to spawn MCP server '{}': {e}", program)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Connector("could not get subprocess stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Connector("could not get subprocess stdout".to_string()))?;

        let mut conn = McpConnection {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };

        let init_params = serde_json::to_value(InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: ClientCapabilities {
                roots: RootsCapability { list_changed: false },
            },
            client_info: ClientInfo {
                name: "openmind-desktop",
                version: env!("CARGO_PKG_VERSION"),
            },
        })
        .map_err(|e| AppError::Connector(format!("serialize init params: {e}")))?;

        conn.request("initialize", Some(init_params)).await?;
        conn.notify("notifications/initialized", None).await?;
        conns.insert(connector_id.to_string(), conn);

        Ok(ConnectorManifest {
            id: descriptor.id.clone(),
            name: descriptor.name.clone(),
            transport: Transport::Stdio,
            auth_state: ConnectorAuthState::Connected,
            fetch_interval_minutes: 0,
            last_fetch_at: None,
        })
    }

    pub async fn disconnect(&self, connector_id: &str) -> AppResult<()> {
        let mut conns = self.connections.lock().await;
        let mut conn = conns
            .remove(connector_id)
            .ok_or_else(|| AppError::Connector(format!("connector '{connector_id}' is not connected")))?;
        let _ = conn.stdin.shutdown().await;
        let _ = conn.child.start_kill();
        Ok(())
    }

    pub async fn list_tools(&self, connector_id: &str) -> AppResult<Vec<McpTool>> {
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(connector_id)
            .ok_or_else(|| AppError::Connector(format!("connector '{connector_id}' is not connected")))?;
        let result = conn.request("tools/list", None).await?;
        let tools_arr = result["tools"]
            .as_array()
            .ok_or_else(|| AppError::Connector("tools/list response missing 'tools' array".to_string()))?;
        Ok(tools_arr
            .iter()
            .map(|t| McpTool {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().map(str::to_string),
            })
            .collect())
    }

    pub async fn call_tool(
        &self,
        connector_id: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> AppResult<CallToolResult> {
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(connector_id)
            .ok_or_else(|| AppError::Connector(format!("connector '{connector_id}' is not connected")))?;

        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments.unwrap_or(Value::Object(Default::default()))
        });
        let result = conn.request("tools/call", Some(params)).await?;
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let content = result["content"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|c| ToolContent {
                        content_type: c["type"].as_str().unwrap_or("text").to_string(),
                        text: c["text"].as_str().map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(CallToolResult { content, is_error })
    }

    pub async fn connected_tools_for_prompt(&self) -> AppResult<Vec<PromptToolInfo>> {
        let mut conns = self.connections.lock().await;
        let mut out = Vec::new();

        for descriptor in &self.descriptors {
            let Some(conn) = conns.get_mut(&descriptor.id) else {
                continue;
            };
            let Ok(result) = conn.request("tools/list", None).await else {
                continue;
            };
            let Some(tools_arr) = result["tools"].as_array() else {
                continue;
            };
            for t in tools_arr {
                out.push(PromptToolInfo {
                    connector_id: descriptor.id.clone(),
                    connector_name: descriptor.name.clone(),
                    tool_name: t["name"].as_str().unwrap_or("").to_string(),
                    description: t["description"].as_str().map(str::to_string),
                });
            }
        }

        Ok(out)
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
