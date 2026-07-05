use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::{AppError, AppResult};

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
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
}

pub const GMAIL_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const KEYCHAIN_SERVICE: &str = "com.openmind.desktop";
const CALLBACK_TIMEOUT_SECONDS: u64 = 180;

fn keychain_account(connector_id: &str) -> String {
    format!("oauth-token:{connector_id}")
}

pub fn store_token(connector_id: &str, token: &OAuthToken) -> AppResult<()> {
    let json = serde_json::to_string(token)
        .map_err(|e| AppError::Connector(format!("serialize token: {e}")))?;
    let entry = keyring_core::Entry::new(KEYCHAIN_SERVICE, &keychain_account(connector_id))
        .map_err(|e| AppError::Connector(format!("keychain entry: {e}")))?;
    entry
        .set_password(&json)
        .map_err(|e| AppError::Connector(format!("keychain write: {e}")))?;
    Ok(())
}

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

pub fn delete_token(connector_id: &str) -> AppResult<()> {
    let entry = keyring_core::Entry::new(KEYCHAIN_SERVICE, &keychain_account(connector_id))
        .map_err(|e| AppError::Connector(format!("keychain entry: {e}")))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Connector(format!("keychain delete: {e}"))),
    }
}

pub async fn run_loopback_flow(
    config: &OAuthConfig,
    connector_id: &str,
    http_client: &reqwest::Client,
) -> AppResult<OAuthToken> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| AppError::Connector(format!("bind loopback listener: {e}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| AppError::Connector(format!("set listener nonblocking: {e}")))?;

    let port = listener
        .local_addr()
        .map_err(|e| AppError::Connector(format!("get local port: {e}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = generate_state();
    let code_verifier = generate_code_verifier();
    let code_challenge = code_challenge(&code_verifier);

    let scope_str = config.scopes.join(" ");
    let auth_url = format!(
        "{GOOGLE_AUTH_URL}?client_id={client_id}&redirect_uri={redirect_uri}&response_type=code&scope={scope}&state={state}&access_type=offline&prompt=consent&code_challenge={code_challenge}&code_challenge_method=S256",
        client_id = percent_encode(&config.client_id),
        redirect_uri = percent_encode(&redirect_uri),
        scope = percent_encode(&scope_str),
        state = state,
        code_challenge = percent_encode(&code_challenge),
    );

    open_browser(&auth_url)?;

    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    std::thread::spawn(move || {
        let result = accept_callback(listener, &state, Duration::from_secs(CALLBACK_TIMEOUT_SECONDS));
        let _ = tx.send(result);
    });

    let auth_code = tokio::time::timeout(Duration::from_secs(CALLBACK_TIMEOUT_SECONDS + 5), rx)
        .await
        .map_err(|_| AppError::Connector("OAuth callback timed out".to_string()))?
        .map_err(|_| AppError::Connector("OAuth callback channel dropped".to_string()))?
        .map_err(AppError::Connector)?;

    let token = exchange_code(
        http_client,
        &config.client_id,
        &config.client_secret,
        &auth_code,
        &redirect_uri,
        &code_verifier,
    )
    .await?;

    store_token(connector_id, &token)?;
    Ok(token)
}

fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn code_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn open_browser(url: &str) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|e| AppError::Connector(format!("open browser: {e}")))?;

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    compile_error!("open_browser: unsupported target OS — add a case for this platform");

    Ok(())
}

fn accept_callback(
    listener: TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let reader = BufReader::new(&stream);
                let request_line = reader
                    .lines()
                    .next()
                    .ok_or("empty request")?
                    .map_err(|e| format!("read request: {e}"))?;

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

                let (html, result): (String, Result<String, String>) = if let Some(error) = params.get("error") {
                    let msg = format!("Authorization denied: {error}");
                    (error_html(&msg), Err(msg))
                } else {
                    let code = params.get("code").ok_or_else(|| "no code in callback".to_string())?;
                    let state = params.get("state").ok_or_else(|| "no state in callback".to_string())?;
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
                return result;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= timeout {
                    return Err("Timed out waiting for OAuth callback. You can try again from the app.".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(format!("accept connection: {err}")),
        }
    }
}

fn success_html() -> &'static str {
    r#"<!DOCTYPE html><html><head><title>OpenMind Desktop</title>
<style>body{font-family:system-ui;max-width:520px;margin:80px auto;text-align:center}
h1{color:#4ade80}p{color:#6b7280}</style></head><body>
<h1>&#10003; Connected</h1>
<p>You can close this tab and return to OpenMind Desktop.</p>
</body></html>"#
}

fn error_html(msg: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><title>OpenMind Desktop</title>
<style>body{{font-family:system-ui;max-width:520px;margin:80px auto;text-align:center}}
h1{{color:#f87171}}p{{color:#6b7280}}</style></head><body>
<h1>Authorization failed</h1><p>{msg}</p>
</body></html>"#
    )
}

async fn exchange_code(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> AppResult<OAuthToken> {
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];

    let response = client
        .post(GOOGLE_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| AppError::Connector(format!("token exchange request: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Connector(format!("token exchange failed {status}: {body}")));
    }

    response
        .json()
        .await
        .map_err(|e| AppError::Connector(format!("parse token response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_url_safe() {
        let verifier = generate_code_verifier();
        let challenge = code_challenge(&verifier);
        assert!(!challenge.contains('='));
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
    }

    #[test]
    fn percent_encoding_works() {
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}
