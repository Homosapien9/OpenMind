use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::model_router::ModelBackend;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub backend: ModelBackend,
    pub model_name: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub ollama_url: Option<String>,
    pub temperature: Option<f32>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            backend: ModelBackend::Ollama,
            model_name: String::new(),
            api_key: None,
            base_url: Some("https://api.openai.com/v1".to_string()),
            ollama_url: Some("http://127.0.0.1:11434".to_string()),
            temperature: Some(0.3),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub provider: ProviderSettings,
    pub onboarding_completed: bool,
}

pub struct SettingsStore {
    path: PathBuf,
    inner: Mutex<AppSettings>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> AppResult<Self> {
        let settings = match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| AppError::Other(anyhow::anyhow!("settings parse error: {e}")))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => AppSettings::default(),
            Err(err) => return Err(AppError::Other(anyhow::anyhow!("settings read error: {err}"))),
        };

        Ok(Self {
            path,
            inner: Mutex::new(settings),
        })
    }

    pub fn get(&self) -> AppResult<AppSettings> {
        self.inner
            .lock()
            .map(|g| g.clone())
            .map_err(|_| AppError::Other(anyhow::anyhow!("settings mutex poisoned")))
    }

    pub fn update(&self, settings: AppSettings) -> AppResult<AppSettings> {
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| AppError::Other(anyhow::anyhow!("settings mutex poisoned")))?;
            *guard = settings.clone();
        }
        self.persist(&settings)?;
        Ok(settings)
    }

    fn persist(&self, settings: &AppSettings) -> AppResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::Other(anyhow::anyhow!("settings dir create error: {e}")))?;
        }

        let tmp = self.path.with_extension("json.tmp");
        let raw = serde_json::to_string_pretty(settings)
            .map_err(|e| AppError::Other(anyhow::anyhow!("settings serialize error: {e}")))?;
        fs::write(&tmp, raw)
            .map_err(|e| AppError::Other(anyhow::anyhow!("settings write error: {e}")))?;
        fs::rename(&tmp, &self.path)
            .map_err(|e| AppError::Other(anyhow::anyhow!("settings rename error: {e}")))?;
        Ok(())
    }
}
