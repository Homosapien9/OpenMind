use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::mcp::{ConnectorRegistry, PromptToolInfo};
use crate::memory::MemoryTree;
use crate::model_router::{ChatMessage, GenerateRequest, ModelRouter};
use crate::settings::ProviderSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LazySource {
    Rule,
    ExactCache,
    SemanticCache,
    Llm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LazyResponse {
    pub text: String,
    pub source: LazySource,
    pub tokens_used: u32,
    pub tokens_saved: u32,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSavingsStats {
    pub total_calls: u64,
    pub cache_hits: u64,
    pub tokens_used: u64,
    pub tokens_saved: u64,
    pub savings_pct: f64,
}

#[derive(Debug, Clone)]
pub struct StreamTarget {
    pub stream_id: String,
    pub app: tauri::AppHandle,
}

const NAIVE_SYSTEM_TOKENS: u32 = 300;
const NAIVE_MEMORY_TOKENS: u32 = 800;
const NAIVE_QUERY_TOKENS: u32 = 200;
const NAIVE_OUTPUT_TOKENS: u32 = 600;
const NAIVE_TOTAL: u32 =
    NAIVE_SYSTEM_TOKENS + NAIVE_MEMORY_TOKENS + NAIVE_QUERY_TOKENS + NAIVE_OUTPUT_TOKENS;
const LAZY_OUTPUT_TOKENS: u32 = 500;
const PLANNER_OUTPUT_TOKENS: u32 = 180;
const DEFAULT_TTL_SECONDS: u64 = 3600;
const CACHE_MAX_SIZE: usize = 500;

fn mini_prompt(intent: &str) -> &'static str {
    match intent {
        "query" => "Answer directly and concisely. No filler.",
        "study" => "Summarise: key points, 3 self-test questions. Be specific.",
        "research" => "Structured analysis: known facts, gaps, next steps.",
        "planning" => "Prioritised task list (HIGH/MED/LOW) + 3 concrete next steps.",
        "summarize" => "TL;DR paragraph + 3-5 bullet takeaways + action items.",
        "reflect" => "One-sentence insight from patterns. Be specific, not generic.",
        "casual" => "Friendly, brief reply.",
        _ => "Be helpful and concise.",
    }
}

fn greeting_response(normalized: &str) -> Option<&'static str> {
    match normalized {
        "hi" => Some("Hey! What would you like to work on?"),
        "hello" => Some("Hello! Ready when you are."),
        "hey" => Some("Hey! What's on your mind?"),
        "good morning" => Some("Good morning! What are we tackling today?"),
        "good evening" => Some("Good evening! What can I help with?"),
        "thanks" => Some("You're welcome!"),
        "thank you" => Some("Happy to help!"),
        "ok" => Some("Got it."),
        "okay" => Some("Got it."),
        "bye" => Some("Goodbye! See you next time."),
        "exit" => Some("Exiting."),
        _ => None,
    }
}

const HELP_PATTERNS: &[&str] = &["help", "what can you do", "commands", "how do you work"];
const STATUS_PATTERNS: &[&str] = &["status", "how are you", "are you ok", "are you running"];

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn rule_response(query: &str) -> Option<&'static str> {
    let norm = normalize(query);

    if let Some(resp) = greeting_response(&norm) {
        return Some(resp);
    }

    let stripped = norm.trim_end_matches(['!', '.', '?', ',']);
    if let Some(resp) = greeting_response(stripped) {
        return Some(resp);
    }

    if HELP_PATTERNS.iter().any(|p| norm.contains(p)) {
        return Some(
            "I can chat, persist conversation history, search memories, and use connected MCP tools when needed.",
        );
    }

    if STATUS_PATTERNS.iter().any(|p| norm.contains(p)) {
        return Some("Running fine. Memory, chat history, and model routing are online.");
    }

    None
}

fn query_hash(query: &str, intent: &str) -> String {
    let normalized = normalize(query);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hasher.update(b"|");
    hasher.update(intent.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn count_tokens(text: &str) -> u32 {
    ((text.len() / 4) as u32).max(1)
}

struct CacheEntry {
    response: String,
    created_at_unix: u64,
    ttl_seconds: u64,
    last_used_seq: u64,
}

impl CacheEntry {
    fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created_at_unix) > self.ttl_seconds
    }
}

struct ExactCache {
    entries: HashMap<String, CacheEntry>,
    max_size: usize,
    seq_counter: u64,
}

impl ExactCache {
    fn new(max_size: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_size,
            seq_counter: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<String> {
        let now = now_unix_seconds();
        let expired = self.entries.get(key).map(|e| e.is_expired(now));
        match expired {
            Some(true) => {
                self.entries.remove(key);
                None
            }
            Some(false) => {
                self.seq_counter += 1;
                let seq = self.seq_counter;
                self.entries.get_mut(key).map(|entry| {
                    entry.last_used_seq = seq;
                    entry.response.clone()
                })
            }
            None => None,
        }
    }

    fn put(&mut self, key: String, response: String) {
        if self.entries.len() >= self.max_size && !self.entries.contains_key(&key) {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used_seq)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }

        self.seq_counter += 1;
        self.entries.insert(
            key,
            CacheEntry {
                response,
                created_at_unix: now_unix_seconds(),
                ttl_seconds: DEFAULT_TTL_SECONDS,
                last_used_seq: self.seq_counter,
            },
        );
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolDirective {
    action: String,
    connector_id: String,
    tool_name: String,
    #[serde(default)]
    arguments: Value,
}

pub struct LazyAgent {
    cache: Mutex<ExactCache>,
    stats: StdMutex<TokenSavingsStats>,
}

impl LazyAgent {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(ExactCache::new(CACHE_MAX_SIZE)),
            stats: StdMutex::new(TokenSavingsStats::default()),
        }
    }

    pub async fn ask(
        &self,
        query: &str,
        intent: &str,
        thread_id: Option<&str>,
        router: &ModelRouter,
        memory: &MemoryTree,
        registry: &ConnectorRegistry,
        provider: &ProviderSettings,
        stream: Option<&StreamTarget>,
    ) -> AppResult<LazyResponse> {
        let t0 = std::time::Instant::now();

        if let Some(rule_text) = rule_response(query) {
            self.record_stats(0, NAIVE_TOTAL);
            return Ok(LazyResponse {
                text: rule_text.to_string(),
                source: LazySource::Rule,
                tokens_used: 0,
                tokens_saved: NAIVE_TOTAL,
                latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
            });
        }

        let cache_key = query_hash(query, intent);
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached_text) = cache.get(&cache_key) {
                self.record_stats(0, NAIVE_TOTAL);
                return Ok(LazyResponse {
                    text: cached_text,
                    source: LazySource::ExactCache,
                    tokens_used: 0,
                    tokens_saved: NAIVE_TOTAL,
                    latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
                });
            }
        }

        let memories = memory.search(query).unwrap_or_default().into_iter().take(5).collect::<Vec<_>>();
        let history = thread_id
            .map(|id| memory.conversation_history(id, 12).unwrap_or_default())
            .unwrap_or_default();
        let tools = registry.connected_tools_for_prompt().await.unwrap_or_default();

        let base_system = self.build_system_prompt(intent, &memories);
        let history_messages = history
            .iter()
            .map(|entry| ChatMessage {
                role: entry.role.clone(),
                content: entry.content.clone(),
            })
            .collect::<Vec<_>>();

        let mut final_messages = vec![ChatMessage {
            role: "system".to_string(),
            content: base_system.clone(),
        }];
        final_messages.extend(history_messages.clone());
        final_messages.push(ChatMessage {
            role: "user".to_string(),
            content: query.to_string(),
        });

        let mut tool_used_summary = None;
        if !tools.is_empty() {
            if let Some(tool_directive) = self
                .plan_tool_use(query, &base_system, &history_messages, &tools, router, provider)
                .await?
            {
                let result = registry
                    .call_tool(
                        &tool_directive.connector_id,
                        &tool_directive.tool_name,
                        Some(tool_directive.arguments.clone()),
                    )
                    .await?;
                let tool_text = result
                    .content
                    .iter()
                    .filter_map(|c| c.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                tool_used_summary = Some(format!(
                    "Used tool {}:{}",
                    tool_directive.connector_id, tool_directive.tool_name
                ));
                final_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Tool result from {}:{}\n{}",
                        tool_directive.connector_id, tool_directive.tool_name, tool_text
                    ),
                });
            }
        }

        let generation = if let Some(stream_target) = stream {
            router
                .generate_stream(
                    GenerateRequest {
                        messages: final_messages.clone(),
                        max_tokens: LAZY_OUTPUT_TOKENS,
                        temperature: provider.temperature,
                    },
                    provider,
                    |delta| emit_stream_delta(stream_target, delta),
                )
                .await?
        } else {
            router
                .generate(
                    GenerateRequest {
                        messages: final_messages.clone(),
                        max_tokens: LAZY_OUTPUT_TOKENS,
                        temperature: provider.temperature,
                    },
                    provider,
                )
                .await?
        };

        let mut text = generation.text;
        if let Some(summary) = tool_used_summary {
            text = format!("{summary}\n\n{text}");
        }
        let tokens_used = generation.tokens_used.max(
            count_tokens(query)
                + count_tokens(&text)
                + final_messages.iter().map(|m| count_tokens(&m.content)).sum::<u32>(),
        );
        let tokens_saved = NAIVE_TOTAL.saturating_sub(tokens_used.min(NAIVE_TOTAL));

        if let Some(thread_id) = thread_id {
            let _ = memory.append_conversation_message(thread_id, "user", query, "{}");
            let _ = memory.append_conversation_message(thread_id, "assistant", &text, "{}");
        }

        {
            let mut cache = self.cache.lock().await;
            cache.put(cache_key, text.clone());
        }

        self.record_stats(tokens_used, tokens_saved);

        Ok(LazyResponse {
            text,
            source: LazySource::Llm,
            tokens_used,
            tokens_saved,
            latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn build_system_prompt(&self, intent: &str, memories: &[crate::memory::MemoryNode]) -> String {
        let mut prompt = String::from(mini_prompt(intent));
        prompt.push_str("\nUse any supplied memory/context when it is relevant, but do not invent facts.");
        if !memories.is_empty() {
            prompt.push_str("\n\nRelevant memory context:\n");
            for mem in memories {
                prompt.push_str(&format!("- [{}] {}\n", mem.source, mem.content.replace('\n', " ")));
            }
        }
        prompt
    }

    async fn plan_tool_use(
        &self,
        query: &str,
        base_system: &str,
        history_messages: &[ChatMessage],
        tools: &[PromptToolInfo],
        router: &ModelRouter,
        provider: &ProviderSettings,
    ) -> AppResult<Option<ToolDirective>> {
        let mut prompt = String::from(base_system);
        prompt.push_str(
            "\n\nYou may decide whether a connected tool is necessary. If a tool is needed, reply with ONLY strict JSON using this schema:\n{\"action\":\"tool\",\"connectorId\":\"filesystem\",\"toolName\":\"name\",\"arguments\":{}}\nIf no tool is needed, reply with ONLY {\"action\":\"answer\"}.",
        );
        prompt.push_str("\nAvailable tools:\n");
        for tool in tools {
            prompt.push_str(&format!(
                "- connector={} tool={} description={}\n",
                tool.connector_id,
                tool.tool_name,
                tool.description.clone().unwrap_or_else(|| "".to_string())
            ));
        }

        let mut messages = vec![ChatMessage {
            role: "system".to_string(),
            content: prompt,
        }];
        messages.extend_from_slice(history_messages);
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: query.to_string(),
        });

        let planner = router
            .generate(
                GenerateRequest {
                    messages,
                    max_tokens: PLANNER_OUTPUT_TOKENS,
                    temperature: Some(0.0),
                },
                provider,
            )
            .await?;

        let raw = planner.text.trim();
        if raw.contains("\"action\":\"answer\"") || raw.contains("\"action\": \"answer\"") {
            return Ok(None);
        }
        if let Some(value) = extract_json_object(raw) {
            let directive: ToolDirective = serde_json::from_value(value)
                .map_err(|e| AppError::Other(anyhow::anyhow!("tool plan parse error: {e}")))?;
            if directive.action != "tool" {
                return Ok(None);
            }
            return Ok(Some(directive));
        }
        Ok(None)
    }

    fn record_stats(&self, tokens_used: u32, tokens_saved: u32) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_calls += 1;
            if tokens_used == 0 {
                stats.cache_hits += 1;
            }
            stats.tokens_used += tokens_used as u64;
            stats.tokens_saved += tokens_saved as u64;
            let denominator = stats.tokens_used + stats.tokens_saved;
            stats.savings_pct = if denominator > 0 {
                (stats.tokens_saved as f64 / denominator as f64) * 100.0
            } else {
                0.0
            };
        }
    }

    pub async fn stats(&self) -> AppResult<TokenSavingsStats> {
        self.stats
            .lock()
            .map(|s| s.clone())
            .map_err(|_| AppError::Other(anyhow::anyhow!("stats mutex poisoned")))
    }
}

fn emit_stream_delta(target: &StreamTarget, delta: &str) -> AppResult<()> {
    use tauri::Emitter;
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DeltaPayload<'a> {
        stream_id: &'a str,
        delta: &'a str,
    }
    target
        .app
        .emit(
            "chat-stream-delta",
            DeltaPayload {
                stream_id: &target.stream_id,
                delta,
            },
        )
        .map_err(|e| AppError::Other(anyhow::anyhow!("stream emit failed: {e}")))
}

fn extract_json_object(raw: &str) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    serde_json::from_str(&raw[start..=end]).ok()
}

impl Default for LazyAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_hash_is_stable() {
        assert_eq!(query_hash(" Hello  world ", "query"), query_hash("hello world", "query"));
    }

    #[test]
    fn exact_cache_lru_round_trip() {
        let mut cache = ExactCache::new(2);
        cache.put("a".into(), "A".into());
        cache.put("b".into(), "B".into());
        assert_eq!(cache.get("a").as_deref(), Some("A"));
        cache.put("c".into(), "C".into());
        assert!(cache.get("b").is_none() || cache.get("a").is_some());
    }

    #[test]
    fn extract_json_object_works() {
        let value = extract_json_object("before {\"action\":\"answer\"} after").unwrap();
        assert_eq!(value["action"], "answer");
    }
}
