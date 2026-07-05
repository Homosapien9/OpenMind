//! LazyAgent — spec §4.
//!
//! Same gate → cache → compress → act pipeline as the Python OpenMind
//! CLI's `lazy_agent.py`, ported to Rust. This file is a direct port of
//! that reference implementation's rule engine and exact-cache layers
//! (Milestone 2, roadmap) — response strings, hash algorithm, and
//! token-budget constants are copied faithfully, not reinvented, since
//! changing them would be an unrequested product decision disguised as
//! a port.
//!
//! STATUS (Milestone 2, roadmap): rule engine and exact cache are real.
//! Semantic cache (embedding-based near-duplicate detection) and full
//! context compression remain `NotImplemented` — both are explicitly
//! scoped out of this milestone in ROADMAP.md ("ship exact-cache + rule
//! engine first as their own sub-milestone; treat semantic cache as
//! optional, not a blocker"). When the LLM path IS reached (rule miss +
//! cache miss), this calls `ModelRouter::generate()` with the matching
//! minimal per-intent system prompt from the Python reference, but
//! without memory-context compression yet (no memories are passed in
//! from this milestone's callers).
//!
//! Deliberately does NOT own a ModelRouter. Tauri's State<T> already
//! manages one shared ModelRouter instance app-wide (see lib.rs's
//! `setup()`); commands.rs passes a router reference into `ask()` at
//! call time instead of LazyAgent holding a second, separate instance.
//! This keeps exactly one ModelRouter (and, once implemented, exactly
//! one connection pool to Ollama/LM Studio) for the whole app.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::model_router::{GenerateRequest, ModelRouter};

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

// ── Token budget constants — copied from lazy_agent.py verbatim ──────────
//
// "Naive" = full system prompt + all memories + full query, the baseline
// every savings percentage is measured against. "Lazy" = the compressed
// budget used once context compression exists (Milestone 2 stretch /
// later milestone) — LAZY_* constants are kept here even though nothing
// reads them yet, so the budget numbers stay co-located with NAIVE_TOTAL
// for whoever implements compression next, rather than scattered.

const NAIVE_SYSTEM_TOKENS: u32 = 300;
const NAIVE_MEMORY_TOKENS: u32 = 800;
const NAIVE_QUERY_TOKENS: u32 = 200;
const NAIVE_OUTPUT_TOKENS: u32 = 600;
const NAIVE_TOTAL: u32 =
    NAIVE_SYSTEM_TOKENS + NAIVE_MEMORY_TOKENS + NAIVE_QUERY_TOKENS + NAIVE_OUTPUT_TOKENS;

/// Token budget for the model output path — used in ask() as the
/// max_tokens hint passed to ModelRouter::generate().
const LAZY_OUTPUT_TOKENS: u32 = 400;

/// Default cache TTL, matching the Python reference's `ttl: float =
/// 3600.0` default parameter on `ask()`.
const DEFAULT_TTL_SECONDS: u64 = 3600;

/// Max entries in the exact-match cache before LRU eviction kicks in —
/// matches Python's `CACHE_MAX_SIZE`.
const CACHE_MAX_SIZE: usize = 500;

// ── Minimal system prompts per intent — copied from _MINI_PROMPTS ────────

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

// ── Rule engine — copied from _GREETING_RESPONSES / _rule_response ───────
//
// Response strings are copied verbatim from the Python reference. This
// is a deliberate choice: porting "the same feature" means the same
// behavior, not a rewrite with different wording — changing copy here
// would be an unrequested product decision smuggled into a port.

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

/// Lowercase, trim, and collapse internal whitespace — matches Python's
/// `_normalize()` (`re.sub(r"\s+", " ", text.lower().strip())`).
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns a deterministic answer if a rule matches, else None — matches
/// Python's `_rule_response()` exactly, including the "strip trailing
/// punctuation and retry" fallback for greetings.
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
            "I can: chat, study, research, plan, summarise, ingest files, \
             search memory, reflect, and take autonomous actions.\n\
             Commands: chat, ingest <file>, search <query>, plan, status, reflect, help, exit",
        );
    }

    if STATUS_PATTERNS.iter().any(|p| norm.contains(p)) {
        return Some("Running fine. Memory and model ready.");
    }

    None
}

/// SHA-256(normalized_query + "|" + intent) — matches Python's
/// `_query_hash()` exactly, including the "|" separator.
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

/// Fast token estimate: ~4 chars per token — matches Python's
/// `_count_tokens()` exactly (`max(1, len(text) // 4)`).
fn count_tokens(text: &str) -> u32 {
    ((text.len() / 4) as u32).max(1)
}

// ── Exact-match cache ──────────────────────────────────────────────────

struct CacheEntry {
    response: String,
    created_at_unix: u64,
    ttl_seconds: u64,
    /// LRU recency counter — see ExactCache::get/put for how this is
    /// used instead of a Python-style list-reordering approach (a
    /// HashMap doesn't preserve insertion order the way Python's dict +
    /// separate `_order` list does, so recency is tracked per-entry and
    /// eviction scans for the minimum, which is equivalent behavior for
    /// CACHE_MAX_SIZE=500 — not so large that an O(n) scan on eviction
    /// matters).
    last_used_seq: u64,
}

impl CacheEntry {
    fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created_at_unix) > self.ttl_seconds
    }
}

/// In-memory exact-match response cache with TTL + LRU eviction. Matches
/// the `_exact` half of Python's `ResponseCache` — the `_semantic` half
/// (embedding index) is not ported yet; see this module's doc comment.
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
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.last_used_seq = seq;
                    Some(entry.response.clone())
                } else {
                    None
                }
            }
            None => None,
        }
    }

    fn put(&mut self, key: String, response: String) {
        if self.entries.len() >= self.max_size && !self.entries.contains_key(&key) {
            // Evict the least-recently-used entry — equivalent to
            // Python's "evict self._order[0]" but via a min-scan over
            // last_used_seq instead of a maintained order list.
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

/// The cache field uses `tokio::sync::Mutex` because it's locked around
/// an `.await` (the `router.generate()` call on cache miss). The stats
/// field uses `std::sync::Mutex` because it's only ever locked after all
/// `.await` points complete — no guard is ever held across an await, so
/// the cheaper sync mutex is correct and avoids unnecessary async overhead.
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

    /// Run a query through gate → cache → compress → act.
    ///
    /// `router` is the app's single shared ModelRouter (see this
    /// module's doc comment) — passed in by the caller (commands.rs,
    /// pulling it from Tauri's managed state) rather than owned here.
    ///
    /// Milestone 2 scope: rule engine (step 1) and exact cache (step 2)
    /// are real. Semantic cache (step 3) is skipped entirely — not
    /// attempted and not a silent no-op, just genuinely absent, per the
    /// roadmap's explicit deferral. Step 4 (LLM call) has no memory
    /// context compression yet since no memories are threaded through
    /// from this milestone's callers; it uses the matching minimal
    /// system prompt and calls `router.generate()` directly.
    pub async fn ask(
        &self,
        query: &str,
        intent: &str,
        router: &ModelRouter,
    ) -> AppResult<LazyResponse> {
        let t0 = std::time::Instant::now();

        // -- 1. RULE ENGINE (0 tokens) ----------------------------------
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

        // -- 2. EXACT CACHE HIT (0 tokens) -------------------------------
        let cache_key = query_hash(query, intent);
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached_text) = cache.get(&cache_key) {
                drop(cache);
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

        // -- 3. SEMANTIC CACHE — not implemented this milestone ---------
        // (intentionally absent — see module doc comment)

        // -- 4. LLM CALL (no compression yet — see doc comment) ---------
        let system_prompt = mini_prompt(intent);
        let response = router
            .generate(GenerateRequest {
                prompt: query.to_string(),
                system_prompt: Some(system_prompt.to_string()),
                max_tokens: LAZY_OUTPUT_TOKENS,
            })
            .await?;

        let tokens_used = count_tokens(system_prompt) + count_tokens(query) + count_tokens(&response.text);
        let tokens_saved = NAIVE_TOTAL.saturating_sub(tokens_used);

        // -- 5. CACHE STORE ------------------------------------------------
        {
            let mut cache = self.cache.lock().await;
            cache.put(cache_key, response.text.clone());
        }

        self.record_stats(tokens_used, tokens_saved);

        Ok(LazyResponse {
            text: response.text,
            source: LazySource::Llm,
            tokens_used,
            tokens_saved,
            latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn record_stats(&self, tokens_used: u32, tokens_saved: u32) {
        // std::sync::Mutex — safe here because this is called only after
        // all .await points in ask() have completed.
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

impl Default for LazyAgent {
    fn default() -> Self {
        Self::new()
    }
}
