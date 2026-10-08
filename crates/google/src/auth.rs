use crate::store::{CredentialStore, NativeStore};
use freesync_core::{Error, ErrorCode, Result};
use oauth2::{CsrfToken, PkceCodeChallenge, PkceCodeVerifier};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn config_directory() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.config_dir().into()))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("freesync")
}
pub fn bootstrap_account() -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(
        config_directory().join("oauth-bootstrap.json"),
    )?)?;
    value["account"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(Into::into)
        .ok_or_else(|| Error::new(ErrorCode::Authentication, "Connect a Google account first."))
}

/// Secret values are deliberately neither Debug nor exposed through Tauri commands.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub token: Option<String>,
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "token_uri")]
    pub token_uri: String,
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub scopes: Vec<String>,
}
fn token_uri() -> String {
    TOKEN_URL.into()
}
impl Credentials {
    pub fn validate(&self) -> Result<()> {
        if self.refresh_token.is_empty()
            || self.client_id.is_empty()
            || self.token_uri != TOKEN_URL
            || !self.scopes.iter().any(|s| s == SCOPE)
        {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Saved Google authorization is invalid or lacks Drive access. Reconnect.",
            ));
        }
        Ok(())
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    token_type: String,
}

pub struct Auth {
    pub account: String,
    credentials: Mutex<Credentials>,
    store: Arc<dyn CredentialStore>,
    client: reqwest::Client,
}
impl Auth {
    pub async fn saved(account: &str) -> Result<Self> {
        Self::with_store(account, Arc::new(NativeStore)).await
    }
    pub async fn with_store(account: &str, store: Arc<dyn CredentialStore>) -> Result<Self> {
        let raw = store.load(account).await?.ok_or_else(|| {
            Error::new(
                ErrorCode::Authentication,
                "No saved authorization for this account. Connect Google Drive.",
            )
        })?;
        let creds: Credentials = serde_json::from_str(&raw)?;
        creds.validate()?;
        Ok(Self {
            account: account.into(),
            credentials: Mutex::new(creds),
            store,
            client: http_client()?,
        })
    }
    pub async fn bearer(&self, force: bool) -> Result<String> {
        let mut creds = self.credentials.lock().await;
        if !force
            && creds.expires_at > now() + 60
            && let Some(token) = &creds.token
        {
            return Ok(token.clone());
        }
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", creds.client_id.as_str()),
                ("client_secret", creds.client_secret.as_str()),
                ("refresh_token", creds.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(network_error)?;
        let token = parse_token(response).await?;
        creds.token = Some(token.access_token.clone());
        creds.expires_at = now() + token.expires_in.unwrap_or(3600);
        if let Some(refresh) = token.refresh_token {
            creds.refresh_token = refresh;
        }
        self.store
            .save(&self.account, &serde_json::to_string(&*creds)?)
            .await?;
        Ok(token.access_token)
    }
    pub async fn disconnect(&self) -> Result<()> {
        let creds = self.credentials.lock().await;
        let response = self
            .client
            .post("https://oauth2.googleapis.com/revoke")
            .form(&[("token", creds.refresh_token.as_str())])
            .send()
            .await
            .map_err(network_error)?;
        // Invalid/already revoked tokens are equivalent to successful disconnection.
        if response.status().is_success() || response.status() == reqwest::StatusCode::BAD_REQUEST {
            self.store.delete(&self.account).await?;
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::Transient,
                "Google could not revoke access. Retry when online.",
            ))
        }
    }
}
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("FreeSync/0.1")
        .build()
        .map_err(network_error)
}
pub fn network_error(_: reqwest::Error) -> Error {
    Error::new(
        ErrorCode::Transient,
        "Google could not be reached. Check the connection; pending work is retained.",
    )
}
async fn parse_token(response: reqwest::Response) -> Result<TokenResponse> {
    if response.status().as_u16() >= 500 {
        return Err(Error::new(
            ErrorCode::Transient,
            "Google's authorization service is temporarily unavailable. Retry when online.",
        ));
    }
    if response.status().as_u16() == 429 {
        return Err(Error::new(
            ErrorCode::RateLimited,
            "Google is temporarily limiting authorization requests.",
        ));
    }
    if !response.status().is_success() {
        return Err(Error::new(
            ErrorCode::Authentication,
            "Google authorization expired or was revoked. Reconnect this account.",
        ));
    }
    let token: TokenResponse = response.json().await.map_err(|_| {
        Error::new(
            ErrorCode::Authentication,
            "Google returned an invalid token response.",
        )
    })?;
    if !token.token_type.eq_ignore_ascii_case("bearer")
        || token.access_token.is_empty()
        || token
            .scope
            .as_deref()
            .is_some_and(|s| !s.split_whitespace().any(|s| s == SCOPE))
    {
        return Err(Error::new(
            ErrorCode::Authentication,
            "Google did not grant the required Drive permission.",
        ));
    }
    Ok(token)
}

#[derive(Deserialize)]
struct ClientConfig {
    installed: Option<DesktopClient>,
}
#[derive(Deserialize)]
struct DesktopClient {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}
pub struct PendingLogin {
    pub url: String,
    listener: TcpListener,
    client: DesktopClient,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
    expected_account: String,
}
impl PendingLogin {
    pub async fn begin(client_path: &Path, account: &str) -> Result<Self> {
        let config: ClientConfig = serde_json::from_slice(&std::fs::read(client_path)?)?;
        let client = config.installed.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "Choose a Google Desktop app OAuth client.",
            )
        })?;
        if client.auth_uri != "https://accounts.google.com/o/oauth2/auth"
            || client.token_uri != TOKEN_URL
        {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "OAuth client configuration contains unexpected endpoints.",
            ));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let redirect = format!("http://127.0.0.1:{}/", listener.local_addr()?.port());
        let state = CsrfToken::new_random();
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut url = reqwest::Url::parse(&client.auth_uri)
            .map_err(|_| Error::new(ErrorCode::Internal, "Invalid authorization endpoint."))?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", client.client_id.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("scope", SCOPE),
            ("state", state.secret()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("login_hint", account),
        ]);
        Ok(Self {
            url: url.to_string(),
            listener,
            client,
            state,
            verifier,
            expected_account: account.into(),
        })
    }
    pub async fn finish(
        self,
        store: Arc<dyn CredentialStore>,
        cancellation: CancellationToken,
    ) -> Result<String> {
        let timeout = tokio::time::sleep(Duration::from_secs(900));
        tokio::pin!(timeout);
        let port = self.listener.local_addr()?.port();
        let code = loop {
            let (mut stream, _) = tokio::select! {
                _=cancellation.cancelled()=>return Err(Error::new(ErrorCode::Cancelled,"Google sign-in was cancelled.")),
                _=&mut timeout=>return Err(Error::new(ErrorCode::Authentication,"Google sign-in timed out. Start again.")),
                accepted=self.listener.accept()=>accepted?,
            };
            let request =
                tokio::time::timeout(Duration::from_secs(5), read_callback_request(&mut stream))
                    .await;
            let parsed = match request {
                Ok(Ok(request)) => validate_callback(&request, port, self.state.secret()),
                _ => Err(Error::new(
                    ErrorCode::Authentication,
                    "Invalid or incomplete sign-in callback.",
                )),
            };
            let (ok, message) = match &parsed {
                Ok(_) => (
                    true,
                    "Google consent received. FreeSync is checking your account. You may close this tab.",
                ),
                Err(_) => (
                    false,
                    "Invalid callback. Return to FreeSync and try signing in again.",
                ),
            };
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                if ok { "200 OK" } else { "400 Bad Request" },
                message.len(),
                message
            );
            // A stray or disconnected browser request cannot cancel a valid pending login.
            let _ = stream.write_all(response.as_bytes()).await;
            if parsed
                .as_ref()
                .is_err_and(|e| e.code == ErrorCode::Cancelled)
            {
                return Err(Error::new(
                    ErrorCode::Cancelled,
                    "Google consent was declined.",
                ));
            }
            if let Ok(code) = parsed {
                break code;
            }
        };
        let client = http_client()?;
        let redirect = format!("http://127.0.0.1:{port}/");
        let response = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client.client_id.as_str()),
                ("client_secret", self.client.client_secret.as_str()),
                ("code", code.as_str()),
                ("code_verifier", self.verifier.secret()),
                ("redirect_uri", redirect.as_str()),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(network_error)?;
        let token = parse_token(response).await?;
        let refresh = token.refresh_token.ok_or_else(|| {
            Error::new(
                ErrorCode::Authentication,
                "Google did not return a refresh token. Repeat consent.",
            )
        })?;
        // Confirm identity before storing a persistent grant under an account key.
        let response = client
            .get("https://www.googleapis.com/drive/v3/about")
            .query(&[("fields", "user(emailAddress)")])
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(network_error)?;
        if !response.status().is_success() {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Google account identity could not be checked.",
            ));
        }
        let identity: serde_json::Value = response.json().await.map_err(network_error)?;
        let email = identity["user"]["emailAddress"].as_str().ok_or_else(|| {
            Error::new(
                ErrorCode::Authentication,
                "Google account identity is missing.",
            )
        })?;
        if !email.eq_ignore_ascii_case(&self.expected_account) {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Signed-in Google account does not match the selected account.",
            ));
        }
        let creds = Credentials {
            token: Some(token.access_token),
            refresh_token: refresh,
            client_id: self.client.client_id,
            client_secret: self.client.client_secret,
            token_uri: TOKEN_URL.into(),
            expires_at: now() + token.expires_in.unwrap_or(3600),
            scopes: vec![SCOPE.into()],
        };
        store.save(email, &serde_json::to_string(&creds)?).await?;
        Ok(email.into())
    }
}

async fn read_callback_request(stream: &mut tokio::net::TcpStream) -> Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Incomplete sign-in callback.",
            ));
        }
        request.extend_from_slice(&chunk[..n]);
        if request.len() > 8192 {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Oversized sign-in callback.",
            ));
        }
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8(request).map_err(|_| {
                Error::new(
                    ErrorCode::Authentication,
                    "Invalid sign-in callback encoding.",
                )
            });
        }
    }
}

pub fn validate_callback(request: &str, port: u16, expected_state: &str) -> Result<String> {
    let invalid = || {
        Error::new(
            ErrorCode::Authentication,
            "Invalid OAuth callback or state.",
        )
    };
    let mut lines = request.lines();
    let line = lines.next().ok_or_else(invalid)?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("GET") {
        return Err(invalid());
    }
    let target = parts.next().ok_or_else(invalid)?;
    if !matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err(invalid());
    }
    if parts.next().is_some() || !target.starts_with("/?") {
        return Err(invalid());
    }
    let expected_host = format!("127.0.0.1:{port}");
    if !lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("host") && value.trim() == expected_host
        })
    }) {
        return Err(invalid());
    }
    let url =
        reqwest::Url::parse(&format!("http://127.0.0.1:{port}{target}")).map_err(|_| invalid())?;
    if url.path() != "/" {
        return Err(invalid());
    }
    let codes: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .collect();
    let states: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .collect();
    if states.len() != 1 || !bool::from(states[0].as_bytes().ct_eq(expected_state.as_bytes())) {
        return Err(invalid());
    }
    let errors: Vec<_> = url.query_pairs().filter(|(k, _)| k == "error").collect();
    if codes.is_empty() && errors.len() == 1 && errors[0].1 == "access_denied" {
        return Err(Error::new(
            ErrorCode::Cancelled,
            "Google consent was declined.",
        ));
    }
    if codes.len() != 1 || codes[0].is_empty() || !errors.is_empty() {
        return Err(invalid());
    }
    Ok(codes[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    struct MemoryStore {
        value: Option<String>,
        unavailable: bool,
    }
    #[async_trait]
    impl CredentialStore for MemoryStore {
        async fn load(&self, _: &str) -> Result<Option<String>> {
            if self.unavailable {
                Err(Error::new(
                    ErrorCode::Authentication,
                    "Locked credential store.",
                ))
            } else {
                Ok(self.value.clone())
            }
        }
        async fn save(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn delete(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    fn credentials() -> Credentials {
        Credentials {
            token: Some("fixture-access-token".into()),
            refresh_token: "fixture-refresh-token".into(),
            client_id: "fixture-client".into(),
            client_secret: "fixture-secret".into(),
            token_uri: TOKEN_URL.into(),
            expires_at: now() + 3600,
            scopes: vec![SCOPE.into()],
        }
    }
    #[test]
    fn callback_rejects_state_injection_duplicate_codes_and_host_spoofing() {
        let valid =
            "GET /?code=fixture-code&state=expected HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n";
        assert_eq!(
            validate_callback(valid, 12345, "expected").unwrap(),
            "fixture-code"
        );
        for request in [
            valid.replace("state=expected", "state=attacker"),
            valid.replace("code=fixture-code", "code=one&code=two"),
            valid.replace("127.0.0.1:12345", "evil.example:12345"),
            valid.replace("GET /?", "GET /other?"),
            valid.replace("GET ", "POST "),
        ] {
            assert_eq!(
                validate_callback(&request, 12345, "expected")
                    .err()
                    .unwrap()
                    .code,
                ErrorCode::Authentication
            );
        }
    }
    #[test]
    fn credentials_reject_endpoint_substitution_and_missing_scope() {
        let mut c = credentials();
        c.validate().unwrap();
        c.token_uri = "https://attacker.invalid/token".into();
        assert!(c.validate().is_err());
        c.token_uri = TOKEN_URL.into();
        c.scopes.clear();
        assert!(c.validate().is_err());
        c.scopes.push(SCOPE.into());
        c.refresh_token.clear();
        assert!(c.validate().is_err());
    }
    #[tokio::test]
    async fn locked_or_missing_store_does_not_fall_back_to_plaintext() {
        for store in [
            MemoryStore {
                value: None,
                unavailable: false,
            },
            MemoryStore {
                value: None,
                unavailable: true,
            },
        ] {
            assert_eq!(
                Auth::with_store("fixture@example.invalid", Arc::new(store))
                    .await
                    .err()
                    .unwrap()
                    .code,
                ErrorCode::Authentication
            );
        }
        let auth = Auth::with_store(
            "fixture@example.invalid",
            Arc::new(MemoryStore {
                value: Some(serde_json::to_string(&credentials()).unwrap()),
                unavailable: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(auth.bearer(false).await.unwrap(), "fixture-access-token");
    }
    #[tokio::test]
    async fn denied_revoked_and_unavailable_authorization_responses_are_classified() {
        for (status, code) in [
            (400, ErrorCode::Authentication),
            (500, ErrorCode::Transient),
            (429, ErrorCode::RateLimited),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                assert!(stream.read(&mut bytes).await.unwrap() > 0);
                let response = format!(
                    "HTTP/1.1 {status} Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let response = http_client()
                .unwrap()
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(parse_token(response).await.err().unwrap().code, code);
        }
    }
}
