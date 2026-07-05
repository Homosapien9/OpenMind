//! Local Model Router — spec §3.
//!
//! Routes generation requests to one of three backends, in priority order:
//!   1. Embedded llama.cpp  — zero-config, bundled, works on first install
//!   2. Ollama               — bring-your-own, larger local models
//!   3. LM Studio             — bring-your-own, alternative to Ollama
//!
//! No cloud backend exists in this enum by design (spec §3): "no cloud
//! model option in v1" means absent from the type system, not merely
//! unconfigured.
//!
//! STATUS (Milestone 1, roadmap): Ollama backend is real — talks to a
//! local Ollama server over HTTP via `/api/chat`, blocking (non-streamed)
//! request/response, no error-state polish beyond what's needed to not
//! crash. Embedded llama.cpp and LM Studio remain `NotImplemented` by
//! design — the roadmap is explicit that Milestone 1 is Ollama-only,
//! scope creep into the other two backends is exactly what it warns
//! against. `status()` only probes Ollama for the same reason.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    Embedded,
    Ollama,
    // Explicit rename rather than relying on snake_case's handling of
    // consecutive capitals (LmStudio) — verified uncertain, not worth
    // risking a silent mismatch with the TS-side ModelBackend union
    // type ("embedded" | "ollama" | "lm_studio") in src/lib/ipc.ts.
    #[serde(rename = "lm_studio")]
    LmStudio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub backend: ModelBackend,
    pub model_name: String,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub prompt: String,
    pub system_prompt: Option<String>,
    /// Accepted for future backends (e.g. embedded llama.cpp's own
    /// context-window limit) but not yet sent to Ollama — Ollama's
    /// /api/chat doesn't take a max-token cap in the same way; that
    /// would map to `options.num_predict`, intentionally left out of
    /// this Milestone 1 scope along with everything else under
    /// `options` (temperature, etc.) per the roadmap's "nothing else"
    /// instruction.
    pub max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub text: String,
    pub tokens_used: u32,
}

// ── Ollama API wire types ───────────────────────────────────────────────
//
// Matches https://docs.ollama.com/api/chat (POST /api/chat) exactly.
// Only the fields this module actually reads/writes are modeled — Ollama's
// real response has more fields (eval_count, eval_duration, logprobs,
// etc.) that serde will silently ignore on deserialize, which is the
// correct behavior here (no #[serde(deny_unknown_fields)]).

#[derive(Debug, Serialize)]
struct OllamaChatRequestMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatRequestMessage>,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponseMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
    /// Total tokens fed to the model for this request — the closest
    /// Ollama field to a "tokens used" figure. Not present on every
    /// response shape in theory (hence Option), though in practice
    /// stream:false chat responses always include it.
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelEntry {
    name: String,
}

const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";
/// No model is hardcoded as "the" default beyond this fallback label —
/// see `status()`: the actual model name comes from whatever Ollama
/// reports as installed, not a guess.
const FALLBACK_MODEL_LABEL: &str = "none installed";

/// The model router. Holds the priority-ordered list of backends and
/// picks the first one that reports itself available.
///
/// `cached_model` stores the last successfully probed model name so
/// `generate()` doesn't need a separate `status()` round-trip — it uses
/// the cached name if present, and only re-probes on first call or when
/// Ollama reports the previous model no longer exists.
pub struct ModelRouter {
    http: reqwest::Client,
    cached_model: std::sync::Mutex<Option<String>>,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            cached_model: std::sync::Mutex::new(None),
        }
    }

    /// Expose the shared HTTP client so other modules (OAuth, future connectors)
    /// can reuse the same connection pool rather than creating a second client.
    pub fn http_client(&self) -> reqwest::Client {
        self.http.clone()
    }

    /// Report the currently-active backend and whether it's reachable.
    ///
    /// Milestone 1 scope: Ollama only. Probes `GET /api/tags` (the
    /// lightest Ollama endpoint that confirms the server is up and
    /// tells us what's actually installed). Also updates the internal
    /// model-name cache used by `generate()`.
    pub async fn status(&self) -> AppResult<ModelStatus> {
        let url = format!("{OLLAMA_BASE_URL}/api/tags");

        let response = match self.http.get(&url).send().await {
            Ok(resp) => resp,
            Err(_) => {
                // Ollama not running / not reachable — normal expected
                // state, not an error. Clear the cache so the next
                // generate() will re-probe.
                if let Ok(mut c) = self.cached_model.lock() {
                    *c = None;
                }
                return Ok(ModelStatus {
                    backend: ModelBackend::Ollama,
                    model_name: FALLBACK_MODEL_LABEL.to_string(),
                    available: false,
                });
            }
        };

        if !response.status().is_success() {
            if let Ok(mut c) = self.cached_model.lock() {
                *c = None;
            }
            return Ok(ModelStatus {
                backend: ModelBackend::Ollama,
                model_name: FALLBACK_MODEL_LABEL.to_string(),
                available: false,
            });
        }

        let tags: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed /api/tags response: {e}")))?;

        let model_name = tags
            .models
            .first()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| FALLBACK_MODEL_LABEL.to_string());

        // Update cache.
        if let Ok(mut c) = self.cached_model.lock() {
            *c = if tags.models.is_empty() { None } else { Some(model_name.clone()) };
        }

        Ok(ModelStatus {
            backend: ModelBackend::Ollama,
            model_name,
            available: !tags.models.is_empty(),
        })
    }

    /// Generate a completion via Ollama's /api/chat, non-streamed.
    ///
    /// Uses the cached model name from the last `status()` call to avoid
    /// a redundant HTTP round-trip. Falls back to re-probing only when
    /// the cache is empty (first call, or after a connectivity failure).
    pub async fn generate(&self, req: GenerateRequest) -> AppResult<GenerateResponse> {
        // Try cached model name first to avoid a status() round-trip.
        // The guard is dropped immediately after the match arm — no lock held
        // across the subsequent .await on status() if we miss the cache.
        let cached = match self.cached_model.lock() {
            Ok(guard) => (*guard).clone(),
            Err(_) => None, // poisoned mutex — treat as cache miss
        };

        let model_name = match cached {
            Some(name) => name,
            None => {
                // Cache miss — probe status and populate it.
                let status = self.status().await?;
                if !status.available {
                    return Err(AppError::ModelUnavailable(
                        "Ollama is not reachable at 127.0.0.1:11434, or no models are \
                         installed — run `ollama serve` and `ollama pull <model>`"
                            .to_string(),
                    ));
                }
                status.model_name
            }
        };

        let mut messages = Vec::new();
        if let Some(system) = req.system_prompt {
            messages.push(OllamaChatRequestMessage {
                role: "system",
                content: system,
            });
        }
        messages.push(OllamaChatRequestMessage {
            role: "user",
            content: req.prompt,
        });

        let body = OllamaChatRequest {
            model: model_name,
            messages,
            stream: false,
        };

        let url = format!("{OLLAMA_BASE_URL}/api/chat");
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request to Ollama failed: {e}")))?;

        if !response.status().is_success() {
            let status_code = response.status();
            let body_text = response.text().await.unwrap_or_default();
            // Invalidate cache — the model may have been unloaded.
            if let Ok(mut c) = self.cached_model.lock() {
                *c = None;
            }
            return Err(AppError::ModelUnavailable(format!(
                "Ollama returned {status_code}: {body_text}"
            )));
        }

        let parsed: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed /api/chat response: {e}")))?;

        Ok(GenerateResponse {
            text: parsed.message.content,
            tokens_used: parsed.prompt_eval_count + parsed.eval_count,
        })
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}
