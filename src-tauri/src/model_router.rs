use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::settings::ProviderSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    Ollama,
    OpenAi,
    OpenRouter,
    Anthropic,
    Nvidia,
    Compatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub backend: ModelBackend,
    pub model_name: String,
    pub available: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub text: String,
    pub tokens_used: u32,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

pub struct ModelRouter {
    http: reqwest::Client,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    pub fn http_client(&self) -> reqwest::Client {
        self.http.clone()
    }

    pub async fn status(&self, cfg: &ProviderSettings) -> AppResult<ModelStatus> {
        match cfg.backend {
            ModelBackend::Ollama => self.ollama_status(cfg).await,
            ModelBackend::Anthropic => self.remote_status(cfg, None).await,
            ModelBackend::OpenAi | ModelBackend::OpenRouter | ModelBackend::Nvidia | ModelBackend::Compatible => {
                self.remote_status(cfg, Some(self.compat_models_url(cfg)?)).await
            }
        }
    }

    pub async fn generate(&self, req: GenerateRequest, cfg: &ProviderSettings) -> AppResult<GenerateResponse> {
        match cfg.backend {
            ModelBackend::Ollama => self.generate_ollama(req, cfg).await,
            ModelBackend::Anthropic => self.generate_anthropic(req, cfg).await,
            ModelBackend::OpenAi | ModelBackend::OpenRouter | ModelBackend::Nvidia | ModelBackend::Compatible => {
                self.generate_openai_compatible(req, cfg).await
            }
        }
    }

    pub async fn generate_stream<F>(
        &self,
        req: GenerateRequest,
        cfg: &ProviderSettings,
        mut on_chunk: F,
    ) -> AppResult<GenerateResponse>
    where
        F: FnMut(&str) -> AppResult<()>,
    {
        match cfg.backend {
            ModelBackend::Ollama => self.generate_ollama_stream(req, cfg, &mut on_chunk).await,
            ModelBackend::Anthropic => self.generate_anthropic_stream(req, cfg, &mut on_chunk).await,
            ModelBackend::OpenAi | ModelBackend::OpenRouter | ModelBackend::Nvidia | ModelBackend::Compatible => {
                self.generate_openai_compatible_stream(req, cfg, &mut on_chunk).await
            }
        }
    }

    async fn ollama_status(&self, cfg: &ProviderSettings) -> AppResult<ModelStatus> {
        let base = ollama_url(cfg);
        let url = format!("{base}/api/tags");
        let response = match self.http.get(&url).send().await {
            Ok(resp) => resp,
            Err(err) => {
                return Ok(ModelStatus {
                    backend: ModelBackend::Ollama,
                    model_name: cfg.model_name.clone().if_empty("none configured"),
                    available: false,
                    detail: Some(format!("Ollama not reachable: {err}")),
                })
            }
        };

        if !response.status().is_success() {
            return Ok(ModelStatus {
                backend: ModelBackend::Ollama,
                model_name: cfg.model_name.clone().if_empty("none configured"),
                available: false,
                detail: Some(format!("Ollama returned {}", response.status())),
            });
        }

        let tags: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed Ollama tags response: {e}")))?;

        let configured = cfg.model_name.trim();
        let selected = if configured.is_empty() {
            tags.models.first().map(|m| m.name.clone()).unwrap_or_else(|| "none installed".to_string())
        } else {
            configured.to_string()
        };
        let found = !selected.is_empty() && tags.models.iter().any(|m| m.name == selected);
        let any_models = !tags.models.is_empty();
        Ok(ModelStatus {
            backend: ModelBackend::Ollama,
            model_name: selected,
            available: if configured.is_empty() { any_models } else { found },
            detail: if configured.is_empty() {
                Some("Using first installed Ollama model unless you pin one in settings.".to_string())
            } else if found {
                Some("Configured Ollama model is installed.".to_string())
            } else {
                Some("Configured Ollama model is not installed yet.".to_string())
            },
        })
    }

    async fn remote_status(&self, cfg: &ProviderSettings, models_url: Option<String>) -> AppResult<ModelStatus> {
        let missing = validate_remote_provider(cfg);
        if let Some(detail) = missing {
            return Ok(ModelStatus {
                backend: cfg.backend,
                model_name: cfg.model_name.clone().if_empty("not configured"),
                available: false,
                detail: Some(detail),
            });
        }

        if let Some(url) = models_url {
            match self.remote_get(&url, cfg).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let detail = match resp.json::<OpenAiModelsResponse>().await {
                        Ok(models) => {
                            let configured = cfg.model_name.trim();
                            if configured.is_empty() {
                                Some(format!("{} models visible.", models.data.len()))
                            } else if models.data.iter().any(|m| m.id == configured) {
                                Some("Configured model is visible to this API key.".to_string())
                            } else {
                                Some("API key works, but the configured model was not listed by the provider.".to_string())
                            }
                        }
                        Err(_) => Some("Provider reachable. Model list endpoint returned an unexpected shape.".to_string()),
                    };
                    return Ok(ModelStatus {
                        backend: cfg.backend,
                        model_name: cfg.model_name.clone(),
                        available: true,
                        detail,
                    });
                }
                Ok(resp) => {
                    return Ok(ModelStatus {
                        backend: cfg.backend,
                        model_name: cfg.model_name.clone(),
                        available: false,
                        detail: Some(format!("Provider returned {}", resp.status())),
                    })
                }
                Err(err) => {
                    return Ok(ModelStatus {
                        backend: cfg.backend,
                        model_name: cfg.model_name.clone(),
                        available: false,
                        detail: Some(format!("Provider not reachable: {err}")),
                    })
                }
            }
        }

        Ok(ModelStatus {
            backend: cfg.backend,
            model_name: cfg.model_name.clone(),
            available: true,
            detail: Some("Provider is configured. Status for this backend is validated at generation time.".to_string()),
        })
    }

    async fn generate_ollama(&self, req: GenerateRequest, cfg: &ProviderSettings) -> AppResult<GenerateResponse> {
        let model = self.resolve_ollama_model(cfg).await?;
        let body = serde_json::json!({
            "model": model,
            "messages": req.messages,
            "stream": false,
            "options": {
                "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
                "num_predict": req.max_tokens,
            }
        });
        let url = format!("{}/api/chat", ollama_url(cfg));
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request to Ollama failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ModelUnavailable(format!("Ollama returned {status}: {body}")));
        }

        let value: Value = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed Ollama response: {e}")))?;

        Ok(GenerateResponse {
            text: value["message"]["content"].as_str().unwrap_or_default().to_string(),
            tokens_used: value["prompt_eval_count"].as_u64().unwrap_or(0) as u32
                + value["eval_count"].as_u64().unwrap_or(0) as u32,
        })
    }

    async fn generate_ollama_stream<F>(
        &self,
        req: GenerateRequest,
        cfg: &ProviderSettings,
        on_chunk: &mut F,
    ) -> AppResult<GenerateResponse>
    where
        F: FnMut(&str) -> AppResult<()>,
    {
        let model = self.resolve_ollama_model(cfg).await?;
        let body = serde_json::json!({
            "model": model,
            "messages": req.messages,
            "stream": true,
            "options": {
                "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
                "num_predict": req.max_tokens,
            }
        });
        let url = format!("{}/api/chat", ollama_url(cfg));
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request to Ollama failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ModelUnavailable(format!("Ollama returned {status}: {body}")));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();
        let mut tokens_used = 0u32;

        while let Some(item) = stream.next().await {
            let bytes = item.map_err(|e| AppError::ModelUnavailable(format!("stream read failed: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].trim().to_string();
                buffer = buffer[idx + 1..].to_string();
                if line.is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(&line)
                    .map_err(|e| AppError::ModelUnavailable(format!("bad Ollama stream chunk: {e}")))?;
                if let Some(delta) = value["message"]["content"].as_str() {
                    if !delta.is_empty() {
                        text.push_str(delta);
                        on_chunk(delta)?;
                    }
                }
                tokens_used = value["prompt_eval_count"].as_u64().unwrap_or(0) as u32
                    + value["eval_count"].as_u64().unwrap_or(0) as u32;
            }
        }

        Ok(GenerateResponse { text, tokens_used })
    }

    async fn generate_openai_compatible(&self, req: GenerateRequest, cfg: &ProviderSettings) -> AppResult<GenerateResponse> {
        let body = serde_json::json!({
            "model": cfg.model_name,
            "messages": req.messages,
            "stream": false,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
        });
        let url = self.compat_chat_url(cfg)?;
        let response = self
            .remote_post(&url, cfg)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request failed: {e}")))?;
        parse_openai_completion(response).await
    }

    async fn generate_openai_compatible_stream<F>(
        &self,
        req: GenerateRequest,
        cfg: &ProviderSettings,
        on_chunk: &mut F,
    ) -> AppResult<GenerateResponse>
    where
        F: FnMut(&str) -> AppResult<()>,
    {
        let body = serde_json::json!({
            "model": cfg.model_name,
            "messages": req.messages,
            "stream": true,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
        });
        let url = self.compat_chat_url(cfg)?;
        let response = self
            .remote_post(&url, cfg)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ModelUnavailable(format!("provider returned {status}: {body}")));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();

        while let Some(item) = stream.next().await {
            let bytes = item.map_err(|e| AppError::ModelUnavailable(format!("stream read failed: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].trim().to_string();
                buffer = buffer[idx + 1..].to_string();
                if line.is_empty() || !line.starts_with("data:") {
                    continue;
                }
                let data = line.trim_start_matches("data:").trim();
                if data == "[DONE]" {
                    break;
                }
                let value: Value = serde_json::from_str(data)
                    .map_err(|e| AppError::ModelUnavailable(format!("bad streaming chunk: {e}")))?;
                if let Some(delta) = value["choices"][0]["delta"]["content"].as_str() {
                    if !delta.is_empty() {
                        text.push_str(delta);
                        on_chunk(delta)?;
                    }
                }
            }
        }

        let tokens_used = estimate_tokens_from_text(&text);
        Ok(GenerateResponse { text, tokens_used })
    }

    async fn generate_anthropic(&self, req: GenerateRequest, cfg: &ProviderSettings) -> AppResult<GenerateResponse> {
        let (system, messages) = split_system_messages(req.messages);
        let body = serde_json::json!({
            "model": cfg.model_name,
            "system": system,
            "messages": messages,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
            "stream": false,
        });
        let url = anthropic_messages_url(cfg)?;
        let response = self
            .anthropic_post(&url, cfg)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ModelUnavailable(format!("Anthropic returned {status}: {body}")));
        }

        let value: Value = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed Anthropic response: {e}")))?;
        let text = value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        Ok(GenerateResponse {
            text: text.clone(),
            tokens_used: value["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32
                + value["usage"]["output_tokens"].as_u64().unwrap_or(estimate_tokens_from_text(&text) as u64) as u32,
        })
    }

    async fn generate_anthropic_stream<F>(
        &self,
        req: GenerateRequest,
        cfg: &ProviderSettings,
        on_chunk: &mut F,
    ) -> AppResult<GenerateResponse>
    where
        F: FnMut(&str) -> AppResult<()>,
    {
        let (system, messages) = split_system_messages(req.messages);
        let body = serde_json::json!({
            "model": cfg.model_name,
            "system": system,
            "messages": messages,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature.or(cfg.temperature).unwrap_or(0.3),
            "stream": true,
        });
        let url = anthropic_messages_url(cfg)?;
        let response = self
            .anthropic_post(&url, cfg)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::ModelUnavailable(format!("Anthropic returned {status}: {body}")));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            let bytes = item.map_err(|e| AppError::ModelUnavailable(format!("stream read failed: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].trim().to_string();
                buffer = buffer[idx + 1..].to_string();
                if line.is_empty() || !line.starts_with("data:") {
                    continue;
                }
                let data = line.trim_start_matches("data:").trim();
                if data == "[DONE]" {
                    break;
                }
                let value: Value = serde_json::from_str(data)
                    .map_err(|e| AppError::ModelUnavailable(format!("bad streaming chunk: {e}")))?;
                let event_type = value["type"].as_str().unwrap_or_default();
                if event_type == "content_block_delta" {
                    if let Some(delta) = value["delta"]["text"].as_str() {
                        if !delta.is_empty() {
                            text.push_str(delta);
                            on_chunk(delta)?;
                        }
                    }
                }
            }
        }

        Ok(GenerateResponse {
            text: text.clone(),
            tokens_used: estimate_tokens_from_text(&text),
        })
    }

    async fn resolve_ollama_model(&self, cfg: &ProviderSettings) -> AppResult<String> {
        if !cfg.model_name.trim().is_empty() {
            return Ok(cfg.model_name.trim().to_string());
        }
        let url = format!("{}/api/tags", ollama_url(cfg));
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("Ollama not reachable: {e}")))?;
        if !response.status().is_success() {
            return Err(AppError::ModelUnavailable(format!("Ollama returned {}", response.status())));
        }
        let tags: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| AppError::ModelUnavailable(format!("malformed /api/tags response: {e}")))?;
        tags.models
            .first()
            .map(|m| m.name.clone())
            .ok_or_else(|| AppError::ModelUnavailable("No Ollama models installed. Run `ollama pull <model>` first.".to_string()))
    }

    fn remote_get(&self, url: &str, cfg: &ProviderSettings) -> reqwest::RequestBuilder {
        let builder = self.http.get(url);
        apply_remote_headers(builder, cfg)
    }

    fn remote_post(&self, url: &str, cfg: &ProviderSettings) -> reqwest::RequestBuilder {
        let builder = self.http.post(url);
        apply_remote_headers(builder, cfg)
    }

    fn anthropic_post(&self, url: &str, cfg: &ProviderSettings) -> reqwest::RequestBuilder {
        self.http
            .post(url)
            .header("x-api-key", cfg.api_key.as_deref().unwrap_or_default())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
    }

    fn compat_chat_url(&self, cfg: &ProviderSettings) -> AppResult<String> {
        Ok(format!("{}/chat/completions", self.compat_base_url(cfg)?))
    }

    fn compat_models_url(&self, cfg: &ProviderSettings) -> AppResult<String> {
        Ok(format!("{}/models", self.compat_base_url(cfg)?))
    }

    fn compat_base_url(&self, cfg: &ProviderSettings) -> AppResult<String> {
        base_url_for(cfg)
    }
}

fn parse_openai_text(value: &Value) -> String {
    if let Some(text) = value["choices"][0]["message"]["content"].as_str() {
        return text.to_string();
    }
    if let Some(arr) = value["choices"][0]["message"]["content"].as_array() {
        return arr
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

async fn parse_openai_completion(response: reqwest::Response) -> AppResult<GenerateResponse> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::ModelUnavailable(format!("provider returned {status}: {body}")));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|e| AppError::ModelUnavailable(format!("malformed provider response: {e}")))?;
    let text = parse_openai_text(&value);
    let usage_total = value["usage"]["total_tokens"].as_u64().unwrap_or(estimate_tokens_from_text(&text) as u64) as u32;
    Ok(GenerateResponse { text, tokens_used: usage_total })
}

fn split_system_messages(messages: Vec<ChatMessage>) -> (String, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut out = Vec::new();
    for msg in messages {
        if msg.role == "system" {
            system_parts.push(msg.content);
        } else {
            out.push(serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            }));
        }
    }
    (system_parts.join("\n\n"), out)
}

fn validate_remote_provider(cfg: &ProviderSettings) -> Option<String> {
    if cfg.api_key.as_deref().unwrap_or_default().trim().is_empty() {
        return Some("API key is missing.".to_string());
    }
    if cfg.model_name.trim().is_empty() {
        return Some("Model name is missing.".to_string());
    }
    if matches!(cfg.backend, ModelBackend::Compatible) && cfg.base_url.as_deref().unwrap_or_default().trim().is_empty() {
        return Some("Base URL is required for custom OpenAI-compatible providers.".to_string());
    }
    None
}

fn base_url_for(cfg: &ProviderSettings) -> AppResult<String> {
    let url = match cfg.backend {
        ModelBackend::OpenAi => cfg.base_url.clone().unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        ModelBackend::OpenRouter => cfg.base_url.clone().unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string()),
        ModelBackend::Nvidia => cfg.base_url.clone().unwrap_or_else(|| "https://integrate.api.nvidia.com/v1".to_string()),
        ModelBackend::Compatible => cfg.base_url.clone().unwrap_or_default(),
        ModelBackend::Anthropic => cfg.base_url.clone().unwrap_or_else(|| "https://api.anthropic.com".to_string()),
        ModelBackend::Ollama => ollama_url(cfg),
    };
    let normalized = url.trim_end_matches('/').to_string();
    if normalized.is_empty() {
        return Err(AppError::ModelUnavailable("Provider base URL is empty.".to_string()));
    }
    Ok(normalized)
}

fn anthropic_messages_url(cfg: &ProviderSettings) -> AppResult<String> {
    Ok(format!("{}/v1/messages", base_url_for(cfg)?))
}

fn ollama_url(cfg: &ProviderSettings) -> String {
    cfg.ollama_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn apply_remote_headers(builder: reqwest::RequestBuilder, cfg: &ProviderSettings) -> reqwest::RequestBuilder {
    let builder = builder
        .header("Authorization", format!("Bearer {}", cfg.api_key.as_deref().unwrap_or_default()))
        .header("content-type", "application/json");
    if matches!(cfg.backend, ModelBackend::OpenRouter) {
        builder
            .header("HTTP-Referer", "https://openmind-desktop.local")
            .header("X-Title", "OpenMind Desktop")
    } else {
        builder
    }
}

fn estimate_tokens_from_text(text: &str) -> u32 {
    ((text.len() / 4) as u32).max(1)
}

trait EmptyFallback {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_openrouter_base_url_is_correct() {
        let cfg = ProviderSettings {
            backend: ModelBackend::OpenRouter,
            base_url: None,
            ..ProviderSettings::default()
        };
        let url = base_url_for(&cfg).unwrap();
        assert_eq!(url, "https://openrouter.ai/api/v1");
    }

    #[test]
    fn split_system_messages_moves_system_out() {
        let (system, messages) = split_system_messages(vec![
            ChatMessage { role: "system".into(), content: "You are helpful".into() },
            ChatMessage { role: "user".into(), content: "Hi".into() },
        ]);
        assert!(system.contains("You are helpful"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }
}
