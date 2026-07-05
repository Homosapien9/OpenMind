//! Shared error type for the OpenMind Desktop core.
//!
//! Every `#[tauri::command]` returns `Result<T, AppError>`. Tauri
//! serializes the `Err` variant to the frontend via `Serialize`, so the
//! TypeScript side (see src/lib/ipc.ts) sees a plain string message on
//! failure.

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("model backend unavailable: {0}")]
    ModelUnavailable(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("connector error: {0}")]
    Connector(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// Tauri commands need their error type to implement Serialize so it can
// cross the IPC boundary as JSON.
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
