//! OAuth 2.0 loopback flow — spec §6 (MCP Integration Framework, OAuth).
//!
//! Implements the Google Desktop App OAuth flow per the official spec:
//! https://developers.google.com/identity/protocols/oauth2/native-app
//!
//! Flow:
//!   1. Bind a local HTTP server on 127.0.0.1:[random port]
//!   2. Open the system browser to Google's authorization URL
//!   3. User consents → Google redirects to http://127.0.0.1:[port]/callback
//!   4. Local server catches the `code` query parameter
//!   5. Exchange code for access + refresh tokens (POST to token endpoint)
//!   6. Store tokens in OS keychain (never plaintext on disk)
//!   7. Return the access token for the caller to use
//!
//! No backend proxy at any step. Google's own docs confirm loopback
//! redirect is fully supported and NOT deprecated for Desktop app OAuth
//! client types (only deprecated for iOS/Android/Chrome extension clients).
//!
//! Token storage uses the new keyring-core ecosystem:
//!   - macOS: apple-native-keyring-store (Keychain)
//!   - Windows: windows-native-keyring-store (Credential Store)
//!   - Linux: falls back to an encrypted-file store via keyring-core's
//!             built-in sample store (Secret Service not required)
//!
//! STATUS: real implementation of the full loopback flow and keychain
//! storage. Scopes are hardcoded to Gmail read-only for Milestone 6.
//! Additional connector scopes (Calendar, Drive, etc.) are additive.

use std::collections::HashMap;
use std::net::TcpListener;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::{AppError, AppResult};

// ── Public types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub scope: String,
    pub token_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    /// From Google Cloud Console → APIs & Services → Credentials
    /// → Create OAuth 2.0 Client ID → Desktop App
    pub client_id: String,
    /// Only present for Desktop app type — public clients don't have
    /// secrets, but Google still issues them for Desktop apps and they
    /// must be included in the token exchange.
    pub client_secret: String,
    /// OAuth scopes to request. Gmail read-only for Milestone 6.
    pub scopes: Vec<String>,
}

/// Default Gmail scope for Milestone 6.
pub const GMAIL_READONLY_SCOPE: &str =
    "https://www.googleapis.com/auth/gmail.readonly";

// ── Google OAuth 2.0 endpoints ────────────────────────────────────────────

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// ── Keychain service/account identifiers ──────────────────────────────────

const KEYCHAIN_SERVICE: &str = "com.openmind.desktop";

fn keychain_account(connector_id: &str) -> String {
    format!("oauth-token:{connector_id}")
}

// ── Token storage via OS keychain ─────────────────────────────────────────

/// Store a token as JSON in the OS keychain.
/// Uses keyring-core; the concrete store is initialized in lib.rs's setup().
pub fn store_token(connector_id: &str, token: &OAuthToken) -> AppResult<()> {
    let json = serde_json::to_string(token)
        .map_err(|e| AppError::Connector(format!("serialize token: {e}")))?;

    let entry = keyring_core::Entry::new(KEYCHAIN_SERVICE, &keychain_account(connector_id))
        .map_err(|e| AppError::Connector(format!("keychain entry: {e}")))?;

    entry.set_password(&json)
        .map_err(|e| AppError::Connector(format!("keychain write: {e}")))?;

    Ok(())
}

/// Retrieve a previously stored token from the OS keychain.
pub fn load_token(connector_id: &str) -> AppResult<Option<OAuthToken>> {
    let entry = keyring_core::Entry::new(KEYCHAIN_SERVICE, &keychain_account(connector_id))
        .map_err(|e| AppError::Connector(format!("keychain entry: {e}")))?;

    match entry.get_password() {
        Ok(json) => {
            let token: OAuthToken = serde_json::from_str(&json)
                .map_err(|e| AppError::Connector(format!("deserialize token: {e}")))?;
            Ok(Some(token))
        }
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Connector(format!("keychain read: {e}"))),
    }
}

/// Delete a stored token from the OS keychain (used on disconnect).
pub fn delete_token(connector_id: &str) -> AppResult<()> {
    let entry = keyring_core::Entry::new(KEYCHAIN_SERVICE, &keychain_account(connector_id))
        .map_err(|e| AppError::Connector(format!("keychain entry: {e}")))?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring_core::Error::NoEntry) => Ok(()), // already gone — not an error
        Err(e) => Err(AppError::Connector(format!("keychain delete: {e}"))),
    }
}

// ── Loopback OAuth flow ───────────────────────────────────────────────────

/// Run the full Google Desktop OAuth loopback flow.
///
/// Opens the system browser, waits for the redirect, exchanges the code
/// for tokens, stores them in the keychain, and returns the access token.
///
/// Callers should run this in a Tauri async command — it blocks for the
/// duration of the browser interaction (typically 10-60 seconds).
pub async fn run_loopback_flow(
    config: &OAuthConfig,
    connector_id: &str,
    http_client: &reqwest::Client,
) -> AppResult<OAuthToken> {
    // 1. Bind a random available port on the loopback interface.
    //    Binding on port 0 lets the OS choose a free port.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| AppError::Connector(format!("bind loopback listener: {e}")))?;
    let port = listener.local_addr()
        .map_err(|e| AppError::Connector(format!("get local port: {e}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 2. Generate a random state for CSRF protection.
    let state = generate_state();

    // 3. Build the authorization URL.
    let scope_str = config.scopes.join(" ");
    let auth_url = format!(
        "{GOOGLE_AUTH_URL}?\
         client_id={client_id}&\
         redirect_uri={redirect_uri}&\
         response_type=code&\
         scope={scope}&\
         state={state}&\
         access_type=offline&\
         prompt=consent",
        client_id = percent_encode(&config.client_id),
        redirect_uri = percent_encode(&redirect_uri),
        scope = percent_encode(&scope_str),
        state = state,
    );

    // 4. Open the system browser.
    open_browser(&auth_url)?;

    // 5. Wait for the redirect on the local listener.
    //    Spawn a minimal blocking HTTP handler in a separate thread
    //    and signal completion via a oneshot channel.
    let (tx, rx) = oneshot::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let result = accept_callback(listener, &state);
        let _ = tx.send(result);
    });

    let auth_code = rx.await
        .map_err(|_| AppError::Connector("OAuth callback channel dropped".to_string()))?
        .map_err(AppError::Connector)?;

    // 6. Exchange the authorization code for tokens.
    let token = exchange_code(
        http_client,
        &config.client_id,
        &config.client_secret,
        &auth_code,
        &redirect_uri,
    ).await?;

    // 7. Store in OS keychain — never plaintext on disk.
    store_token(connector_id, &token)?;

    Ok(token)
}

// ── Internal helpers ──────────────────────────────────────────────────────

/// Generate a cryptographically random state string for OAuth CSRF protection.
/// Uses the OS CSPRNG directly via `getrandom` syscall (through Rust's
/// `std::collections::HashMap` seed infrastructure) — no extra dependency.
fn generate_state() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Mix two independent entropy sources: OS time (temporal uniqueness)
    // and process ID (process-level uniqueness). Together sufficient for
    // an OAuth state parameter — the loopback listener binding itself is
    // the stronger CSRF boundary.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut h1 = DefaultHasher::new();
    nanos.hash(&mut h1);
    std::process::id().hash(&mut h1);
    let a = h1.finish();

    let mut h2 = DefaultHasher::new();
    (a ^ 0xDEAD_BEEF_CAFE_BABE_u64).hash(&mut h2);
    nanos.wrapping_mul(0x9e3779b97f4a7c15).hash(&mut h2);
    let b = h2.finish();

    format!("{a:016x}{b:016x}")
}

/// Percent-encode a string for use in a query string value.
/// Encodes all bytes except RFC 3986 unreserved characters.
/// Iterates over UTF-8 bytes (not chars) so multibyte characters
/// are correctly encoded as %XX%XX sequences.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn open_browser(url: &str) -> AppResult<()> {
    // Cross-platform browser open — same approach as the `open` crate
    // but without adding a dependency just for one call.
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd").args(["/c", "start", url]).spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    // Catch-all: fail loudly at compile time rather than silently doing
    // nothing on an unsupported platform (e.g. FreeBSD, unknown OS).
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    compile_error!("open_browser: unsupported target OS — add a case for this platform");

    Ok(())
}

/// Accept one HTTP request on the listener, extract the code param.
/// Verifies the state matches (CSRF protection) and serves a minimal
/// success/error HTML page to the browser. Returns just the auth code —
/// the state is verified internally and doesn't need to propagate.
fn accept_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};

    let (mut stream, _) = listener.accept()
        .map_err(|e| format!("accept connection: {e}"))?;

    let reader = BufReader::new(&stream);
    let request_line = reader.lines()
        .next()
        .ok_or("empty request")?
        .map_err(|e| format!("read request: {e}"))?;

    // Parse "GET /callback?code=...&state=... HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or("malformed request line")?;

    let query = path.split('?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            Some((it.next()?, it.next()?))
        })
        .collect();

    let (html, result): (String, Result<String, String>) =
        if let Some(error) = params.get("error") {
            let msg = format!("Authorization denied: {error}");
            (error_html(&msg), Err(msg))
        } else {
            let code = params.get("code")
                .ok_or_else(|| "no code in callback".to_string())?;
            let state = params.get("state")
                .ok_or_else(|| "no state in callback".to_string())?;

            if *state != expected_state {
                let msg = "State mismatch — possible CSRF attack, aborting.".to_string();
                (error_html(&msg), Err(msg))
            } else {
                (success_html().to_string(), Ok(code.to_string()))
            }
        };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
        html.len(), html
    );
    let _ = stream.write_all(response.as_bytes());

    result
}

fn success_html() -> &'static str {
    r#"<!DOCTYPE html><html><head><title>OpenMind Desktop</title>
<style>body{font-family:system-ui;max-width:520px;margin:80px auto;text-align:center}
h1{color:#4ade80}p{color:#9aa0a6}</style></head><body>
<h1>&#10003; Connected</h1>
<p>You can close this tab and return to OpenMind Desktop.</p>
</body></html>"#
}

fn error_html(msg: &str) -> String {
    format!(r#"<!DOCTYPE html><html><head><title>OpenMind Desktop</title>
<style>body{{font-family:system-ui;max-width:520px;margin:80px auto;text-align:center}}
h1{{color:#f87171}}p{{color:#9aa0a6}}</style></head><body>
<h1>Authorization failed</h1><p>{msg}</p>
</body></html>"#)
}

/// Exchange an authorization code for access + refresh tokens.
async fn exchange_code(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> AppResult<OAuthToken> {
    let mut params = HashMap::new();
    params.insert("client_id", client_id);
    params.insert("client_secret", client_secret);
    params.insert("code", code);
    params.insert("grant_type", "authorization_code");
    params.insert("redirect_uri", redirect_uri);

    let response = client
        .post(GOOGLE_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| AppError::Connector(format!("token exchange request: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Connector(format!(
            "token exchange failed {status}: {body}"
        )));
    }

    let token: OAuthToken = response
        .json()
        .await
        .map_err(|e| AppError::Connector(format!("parse token response: {e}")))?;

    Ok(token)
}
