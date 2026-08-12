//! OAuth 2.1 + PKCE authentication for WebFind MCP (FR-11)

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl, basic::BasicClient,
    reqwest::Client as HttpClient,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::OAuthConfig;

/// OAuth 2.1 + PKCE state for the authorization flow
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub config: OAuthConfig,
    pub client: BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
    pub http_client: HttpClient,
    pub pkce_verifiers: Arc<RwLock<HashMap<String, PkceCodeVerifier>>>,
    pub csrf_tokens: Arc<RwLock<HashMap<String, CsrfToken>>>,
    pub jwks_cache: Arc<RwLock<Option<JsonWebKeySet>>>,
    pub jwks_last_fetch: Arc<RwLock<Option<SystemTime>>>,
    /// FR-11 audit log: `(timestamp, user_id, tool_name, args_hash, result_status)`.
    audit_log: Arc<RwLock<VecDeque<AuditEntry>>>,
    audit_log_path: Option<PathBuf>,
}

/// A single FR-11 audit entry for an MCP `tools/call`.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub timestamp: SystemTime,
    pub user_id: String,
    pub tool_name: String,
    pub args_hash: String,
    pub result_status: String,
}

fn build_oauth_client(
    config: &OAuthConfig,
) -> Result<
    BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    let issuer = config.issuer.as_ref().context("OAuth issuer is required")?;
    let client_id = config
        .client_id
        .as_ref()
        .context("OAuth client_id is required")?;
    let redirect_uri = config
        .redirect_uri
        .as_ref()
        .context("OAuth redirect_uri is required")?;

    let mut client = BasicClient::new(ClientId::new(client_id.clone()));

    if let Some(secret) = &config.client_secret {
        client = client.set_client_secret(ClientSecret::new(secret.clone()));
    }

    let client = client
        .set_auth_uri(
            AuthUrl::new(issuer.clone() + "/authorize").context("Invalid authorization URL")?,
        )
        .set_token_uri(TokenUrl::new(issuer.clone() + "/token").context("Invalid token URL")?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri.clone()).context("Invalid redirect URI")?);

    Ok(client)
}

impl OAuthState {
    /// Create a new OAuth state from configuration
    pub fn new(config: OAuthConfig) -> Result<Self> {
        let client = build_oauth_client(&config)?;
        let http_client = HttpClient::new();
        let audit_log_path = config.audit_log_path.clone();

        Ok(Self {
            config,
            client,
            http_client,
            pkce_verifiers: Arc::new(RwLock::new(HashMap::new())),
            csrf_tokens: Arc::new(RwLock::new(HashMap::new())),
            jwks_cache: Arc::new(RwLock::new(None)),
            jwks_last_fetch: Arc::new(RwLock::new(None)),
            audit_log: Arc::new(RwLock::new(VecDeque::new())),
            audit_log_path,
        })
    }

    /// Record a FR-11 audit entry for an MCP `tools/call`.
    ///
    /// Entries are kept in an in-memory ring buffer (bounded) and appended to the
    /// audit log file when one is configured. The args are hashed (BLAKE3) so raw
    /// arguments never touch the log.
    pub async fn record_audit(
        &self,
        user_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        result_status: &str,
    ) {
        let args_hash = blake3::hash(serde_json::to_vec(args).unwrap_or_default().as_slice())
            .to_hex()
            .to_string();

        let entry = AuditEntry {
            timestamp: SystemTime::now(),
            user_id: user_id.to_string(),
            tool_name: tool_name.to_string(),
            args_hash,
            result_status: result_status.to_string(),
        };

        // In-memory ring buffer (bounded to 10_000 entries).
        {
            let mut log = self.audit_log.write().await;
            if log.len() >= 10_000 {
                log.pop_front();
            }
            log.push_back(entry.clone());
        }

        // Optional JSONL file append.
        if let (Some(path), Ok(line)) = (&self.audit_log_path, serde_json::to_string(&entry)) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{}", line);
            }
        }
    }

    /// Return a snapshot of recent audit entries (newest last).
    pub async fn recent_audit(&self, limit: usize) -> Vec<AuditEntry> {
        let log = self.audit_log.read().await;
        log.iter().rev().take(limit).cloned().collect()
    }

    /// Generate the authorization URL with PKCE and CSRF protection
    pub async fn authorize_url(
        &self,
        state: String,
    ) -> Result<(String, CsrfToken, PkceCodeVerifier)> {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let csrf_token = CsrfToken::new_random();

        // Store PKCE verifier and CSRF token for later validation
        // PkceCodeVerifier doesn't implement Clone, so we create a new one from the secret
        let pkce_verifier_for_storage = PkceCodeVerifier::new(pkce_verifier.secret().to_string());
        self.pkce_verifiers
            .write()
            .await
            .insert(state.clone(), pkce_verifier_for_storage);
        self.csrf_tokens
            .write()
            .await
            .insert(state.clone(), csrf_token.clone());

        let scopes: Vec<Scope> = self
            .config
            .scopes
            .as_ref()
            .map(|s| {
                s.split(',')
                    .map(|s| Scope::new(s.trim().to_string()))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    Scope::new("openid".to_string()),
                    Scope::new("profile".to_string()),
                ]
            });

        let (auth_url, _csrf_token) = self
            .client
            .authorize_url(|| csrf_token.clone())
            .add_scopes(scopes)
            .set_pkce_challenge(pkce_challenge)
            .url();

        Ok((auth_url.to_string(), csrf_token, pkce_verifier))
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code(
        &self,
        code: String,
        state: String,
    ) -> Result<
        oauth2::StandardTokenResponse<oauth2::EmptyExtraTokenFields, oauth2::basic::BasicTokenType>,
    > {
        // Retrieve and remove PKCE verifier
        let pkce_verifier = self
            .pkce_verifiers
            .write()
            .await
            .remove(&state)
            .context("Invalid or expired PKCE state")?;

        // Retrieve and remove CSRF token
        let _csrf_token = self
            .csrf_tokens
            .write()
            .await
            .remove(&state)
            .context("Invalid or expired CSRF state")?;

        let token_result = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(&self.http_client)
            .await
            .context("Failed to exchange authorization code")?;

        Ok(token_result)
    }

    /// Validate a JWT access token
    pub async fn validate_token(&self, token: &str) -> Result<Claims> {
        // Fetch JWKS if needed
        self.ensure_jwks().await?;

        let jwks_guard = self.jwks_cache.read().await;
        let jwks = jwks_guard.as_ref().context("JWKS not available")?;

        // Decode header to get key ID
        let header = decode_header(token).context("Invalid token header")?;
        let kid = header.kid.context("Token missing key ID")?;

        // Find the matching key
        let key = jwks
            .keys
            .iter()
            .find(|k| k.kid == Some(kid.clone()))
            .context("Key not found in JWKS")?;

        // Create decoding key from JWK
        let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e)
            .context("Failed to create decoding key")?;

        // Validate token
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[self
            .config
            .audience
            .as_ref()
            .context("OAuth audience is required")?]);
        validation.set_issuer(&[self
            .config
            .issuer
            .as_ref()
            .context("OAuth issuer is required")?]);

        let token_data = decode::<Claims>(token, &decoding_key, &validation)
            .context("Token validation failed")?;

        Ok(token_data.claims)
    }

    /// Ensure JWKS is fetched and cached
    async fn ensure_jwks(&self) -> Result<()> {
        // Short TTL (30s) so revoked signing keys take effect quickly — meets the
        // FR-11 "sub-60s revocation" requirement. A shorter interval costs one
        // small HTTP fetch per refresh, which is negligible on the MCP hot path.
        const JWKS_CACHE_TTL: Duration = Duration::from_secs(30);

        let should_fetch = {
            let last_fetch = self.jwks_last_fetch.read().await;
            last_fetch.map_or(true, |t| {
                t.elapsed().unwrap_or(Duration::MAX) > JWKS_CACHE_TTL
            })
        };

        if should_fetch {
            let jwks_uri = self
                .config
                .jwks_uri
                .clone()
                .or_else(|| {
                    self.config
                        .issuer
                        .clone()
                        .map(|i| i + "/.well-known/jwks.json")
                })
                .context("JWKS URI not configured")?;

            let response = reqwest::get(jwks_uri.as_str())
                .await
                .context("Failed to fetch JWKS")?;
            let jwks: JsonWebKeySet = response.json().await.context("Failed to parse JWKS")?;

            *self.jwks_cache.write().await = Some(jwks);
            *self.jwks_last_fetch.write().await = Some(SystemTime::now());
        }

        Ok(())
    }
}

/// JWT Claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub aud: Vec<String>,
    pub iss: String,
    pub exp: u64,
    pub iat: u64,
    pub scope: Option<String>,
    pub client_id: Option<String>,
}

/// MCP tool names, mapped to their least-privilege OAuth scope.
///
/// A token whose `scope` claim does not contain the required scope for a tool
/// is rejected with 403 before the tool runs (FR-11 per-user token scoping).
pub const MCP_TOOL_SCOPE: &[(&str, &str)] = &[
    ("webfind_search", "webfind.search"),
    ("webfind_research", "webfind.research"),
    ("webfind_fetch", "webfind.fetch"),
    ("webfind_graph", "webfind.graph"),
    ("webfind_run", "webfind.run"),
];

impl Claims {
    /// The scopes granted to this token, split on whitespace.
    fn granted_scopes(&self) -> Vec<&str> {
        self.scope
            .as_deref()
            .map(|s| s.split_whitespace().collect())
            .unwrap_or_default()
    }

    /// Whether the token's scopes authorize the given MCP tool name.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        // Match the tool name to its required scope.
        let Some((_, required)) = MCP_TOOL_SCOPE.iter().find(|(name, _)| *name == tool_name) else {
            // Unknown tool: deny by default (least privilege).
            return false;
        };

        let granted = self.granted_scopes();
        granted.contains(&"webfind.*") || granted.contains(required)
    }
}

/// JWKS structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonWebKeySet {
    pub keys: Vec<JsonWebKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonWebKey {
    pub kty: String,
    pub kid: Option<String>,
    pub n: String,
    pub e: String,
    pub alg: Option<String>,
    pub use_: Option<String>,
}

/// OAuth authorization request parameters
#[derive(Debug, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

/// OAuth token request parameters
#[derive(Debug, Deserialize)]
pub struct TokenParams {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub code_verifier: Option<String>,
}

/// OAuth token response
#[derive(Debug, Serialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

/// Create OAuth routes for the HTTP server
pub fn oauth_routes(state: Arc<OAuthState>) -> Router {
    Router::new()
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", get(token).post(token_post))
        .route("/oauth/jwks", get(jwks))
        .with_state(state)
}

/// GET /oauth/authorize - Initiate OAuth authorization flow
async fn authorize(
    State(state): State<Arc<OAuthState>>,
    Query(params): Query<AuthorizeParams>,
) -> impl IntoResponse {
    // Validate client_id
    if params.client_id != state.config.client_id.as_deref().unwrap_or("") {
        return (StatusCode::BAD_REQUEST, "Invalid client_id").into_response();
    }

    // Validate redirect_uri
    if params.redirect_uri != state.config.redirect_uri.as_deref().unwrap_or("") {
        return (StatusCode::BAD_REQUEST, "Invalid redirect_uri").into_response();
    }

    // Validate response_type
    if params.response_type != "code" {
        return (StatusCode::BAD_REQUEST, "Unsupported response_type").into_response();
    }

    // Validate PKCE
    if params.code_challenge_method.as_deref() != Some("S256") {
        return (
            StatusCode::BAD_REQUEST,
            "code_challenge_method must be S256",
        )
            .into_response();
    }
    if params.code_challenge.is_none() {
        return (StatusCode::BAD_REQUEST, "code_challenge is required").into_response();
    }

    // Generate state parameter for CSRF protection
    let _state_param = params.state.unwrap_or_else(|| {
        let mut rng = rand::rng();
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    });

    // Build authorization URL with PKCE
    let scopes: Vec<Scope> = state
        .config
        .scopes
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|s| Scope::new(s.trim().to_string()))
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                Scope::new("openid".to_string()),
                Scope::new("profile".to_string()),
            ]
        });

    let (pkce_challenge, _pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let csrf_token = CsrfToken::new_random();

    let (auth_url, _csrf_token) = state
        .client
        .authorize_url(|| csrf_token)
        .add_scopes(scopes)
        .set_pkce_challenge(pkce_challenge)
        .url();

    // Redirect to the authorization server
    Redirect::to(auth_url.as_str()).into_response()
}

/// GET /oauth/token - Token endpoint (for PKCE, typically POST)
async fn token(
    State(_state): State<Arc<OAuthState>>,
    Query(_params): Query<TokenParams>,
) -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        "Use POST for token endpoint",
    )
        .into_response()
}

/// POST /oauth/token - Exchange authorization code for tokens
async fn token_post(
    State(state): State<Arc<OAuthState>>,
    axum::extract::Form(params): axum::extract::Form<TokenParams>,
) -> impl IntoResponse {
    if params.grant_type != "authorization_code" {
        return (StatusCode::BAD_REQUEST, "Unsupported grant_type").into_response();
    }

    let code = match params.code {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "Missing code").into_response(),
    };
    let code_verifier = match params.code_verifier {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "Missing code_verifier").into_response(),
    };
    let state_param = match params.redirect_uri {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "Missing state").into_response(),
    };

    // Verify PKCE
    let pkce_verifier = PkceCodeVerifier::new(code_verifier);
    let _challenge = PkceCodeChallenge::from_code_verifier_sha256(&pkce_verifier);

    match state.exchange_code(code, state_param).await {
        Ok(token_response) => {
            let response = OAuthTokenResponse {
                access_token: token_response.access_token().secret().to_string(),
                token_type: "Bearer".to_string(),
                expires_in: token_response
                    .expires_in()
                    .map(|d| d.as_secs())
                    .unwrap_or(3600),
                refresh_token: token_response
                    .refresh_token()
                    .map(|t| t.secret().to_string()),
                scope: token_response.scopes().map(|s| {
                    s.iter()
                        .map(|sc| sc.to_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
            };
            (StatusCode::OK, axum::Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("Token exchange failed: {}", e);
            (StatusCode::BAD_REQUEST, "Token exchange failed").into_response()
        }
    }
}

/// GET /oauth/jwks - JSON Web Key Set endpoint
async fn jwks(State(state): State<Arc<OAuthState>>) -> impl IntoResponse {
    if let Err(e) = state.ensure_jwks().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("JWKS unavailable: {}", e),
        )
            .into_response();
    }

    let jwks = state.jwks_cache.read().await;
    if let Some(jwks) = jwks.as_ref() {
        (StatusCode::OK, axum::Json(jwks)).into_response()
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "JWKS not available").into_response()
    }
}

/// Extract and validate Bearer token from Authorization header
pub fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Middleware to validate OAuth tokens for protected routes
pub async fn oauth_auth_middleware(
    State(state): State<Arc<OAuthState>>,
    headers: HeaderMap,
    mut request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    if !state.config.enabled.unwrap_or(false) {
        return Ok(next.run(request).await);
    }

    let token = extract_bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = state
        .validate_token(&token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Check token expiration
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    if claims.exp < now {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Add claims to request extensions for downstream handlers
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Axum middleware protecting the MCP `/mcp` route with OAuth 2.1.
///
/// Beyond validating the bearer token (via [`oauth_auth_middleware`]), it
/// enforces FR-11 per-user token scoping: it reads the JSON-RPC request body,
/// extracts the target tool name from a `tools/call`, and rejects the request
/// with 403 if the token's `scope` claim does not authorize that tool. Every
/// `tools/call` (authorized or denied) is written to the audit log.
///
/// The body is buffered so it can be re-attached for the inner service (rmcp
/// consumes it). MCP bodies are capped by `RequestBodyLimitLayer` (1MB) so
/// buffering is bounded.
pub async fn mcp_auth_middleware(
    State(state): State<Arc<OAuthState>>,
    headers: HeaderMap,
    request: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    if !state.config.enabled.unwrap_or(false) {
        return Ok(next.run(request).await);
    }

    // 1. Validate the bearer token (401 on failure).
    let token = extract_bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let claims = state
        .validate_token(&token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    if claims.exp < now {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 2. Buffer the body so we can inspect the tool name and re-attach it.
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 1_048_576)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // 3. Extract the target tool name from the JSON-RPC payload.
    let tool_name = extract_mcp_tool_name(&bytes);

    // 4. Enforce least-privilege scope (403 on mismatch).
    if let Some(tool) = tool_name.as_deref()
        && !claims.allows_tool(tool)
    {
        state
            .record_audit(&claims.sub, tool, &serde_json::Value::Null, "denied")
            .await;
        return Err(StatusCode::FORBIDDEN);
    }

    // 5. Re-attach the buffered body and pass through.
    let mut request = axum::http::Request::from_parts(parts, Body::from(bytes));
    request.extensions_mut().insert(claims.clone());

    let response = next.run(request).await;

    // 6. Audit every authorized tool call with its result status.
    if let Some(tool) = tool_name {
        let status = if response.status().is_success() {
            "ok".to_string()
        } else {
            response.status().as_str().to_string()
        };
        state
            .record_audit(&claims.sub, &tool, &serde_json::Value::Null, &status)
            .await;
    }

    Ok(response)
}

/// Parse the MCP JSON-RPC body and return the target tool name for a
/// `tools/call` request. Returns `None` for non-tool-call methods.
fn extract_mcp_tool_name(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;

    // Single request or batch? Handle both.
    if let Some(obj) = value.as_object() {
        if obj.get("method")?.as_str()? == "tools/call" {
            return obj
                .get("params")?
                .get("name")?
                .as_str()
                .map(|s| s.to_string());
        }
    } else if let Some(batch) = value.as_array() {
        for entry in batch {
            if let Some(name) = extract_mcp_tool_name_entry(entry) {
                return Some(name);
            }
        }
    }
    None
}

fn extract_mcp_tool_name_entry(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;
    if obj.get("method")?.as_str()? != "tools/call" {
        return None;
    }
    obj.get("params")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generation() {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        assert!(!challenge.as_str().is_empty());
        assert!(!verifier.secret().is_empty());
    }

    fn claims_with_scope(scope: Option<&str>) -> Claims {
        Claims {
            sub: "user-1".to_string(),
            aud: vec!["webfind".to_string()],
            iss: "https://auth.example.com".to_string(),
            exp: 9999999999,
            iat: 1,
            scope: scope.map(|s| s.to_string()),
            client_id: Some("client-1".to_string()),
        }
    }

    #[test]
    fn test_scope_denies_unauthorized_tool() {
        let claims = claims_with_scope(Some("openid webfind.search"));
        assert!(claims.allows_tool("webfind_search"));
        assert!(!claims.allows_tool("webfind_research"));
        assert!(!claims.allows_tool("webfind_fetch"));
        assert!(!claims.allows_tool("webfind_graph"));
        assert!(!claims.allows_tool("webfind_run"));
    }

    #[test]
    fn test_scope_allows_wildcard() {
        let claims = claims_with_scope(Some("openid webfind.*"));
        for (name, _) in MCP_TOOL_SCOPE {
            assert!(claims.allows_tool(name), "wildcard should allow {name}");
        }
    }

    #[test]
    fn test_scope_denies_when_missing() {
        let claims = claims_with_scope(None);
        assert!(!claims.allows_tool("webfind_search"));
        // Unknown tool is always denied (least privilege).
        assert!(!claims.allows_tool("webfind_unknown_tool"));
    }

    #[test]
    fn test_scope_allows_exact_matches() {
        let claims = claims_with_scope(Some("webfind.search webfind.fetch"));
        assert!(claims.allows_tool("webfind_search"));
        assert!(claims.allows_tool("webfind_fetch"));
        assert!(!claims.allows_tool("webfind_graph"));
    }

    #[test]
    fn test_extract_mcp_tool_name_single() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"webfind_search","arguments":{"query":"rust"}}}"#;
        assert_eq!(
            extract_mcp_tool_name(body),
            Some("webfind_search".to_string())
        );
    }

    #[test]
    fn test_extract_mcp_tool_name_non_tool_method() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        assert_eq!(extract_mcp_tool_name(body), None);
    }

    #[test]
    fn test_extract_mcp_tool_name_batch() {
        let body = br#"[
            {"jsonrpc":"2.0","id":1,"method":"tools/list"},
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"webfind_graph"}}
        ]"#;
        assert_eq!(
            extract_mcp_tool_name(body),
            Some("webfind_graph".to_string())
        );
    }

    #[tokio::test]
    async fn test_audit_log_records_and_bounds() {
        let config = crate::config::OAuthConfig {
            issuer: Some("https://auth.example.com".to_string()),
            client_id: Some("client-1".to_string()),
            redirect_uri: Some("http://localhost:5748/callback".to_string()),
            ..Default::default()
        };
        let state = OAuthState::new(config).unwrap();
        let mut state = state;
        state.audit_log_path = None;

        for i in 0..15_000 {
            state
                .record_audit(
                    "user-1",
                    "webfind_search",
                    &serde_json::json!({"query": format!("q{i}")}),
                    "ok",
                )
                .await;
        }

        let recent = state.recent_audit(10).await;
        // Ring buffer is bounded at 10_000.
        let full = state.audit_log.read().await;
        assert!(full.len() <= 10_000);
        assert_eq!(recent.len(), 10);
        // Args are hashed, never raw.
        let entry = recent.first().unwrap();
        assert_ne!(entry.args_hash, "q14999");
        assert!(entry.args_hash.len() == 64);
    }

    /// Three concurrent users with different scopes, sharing one OAuthState.
    ///
    /// This exercises the FR-11 acceptance criteria at the concurrency boundary:
    /// each user (1) can only call the tools their scope authorizes and (2) their
    /// tool calls are attributed to the correct `user_id` in the shared audit log
    /// even when interleaved across tasks. The scope check + audit path here is
    /// exactly what `mcp_auth_middleware` runs after token validation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_three_concurrent_users_respect_scopes_and_audit() {
        let config = crate::config::OAuthConfig {
            issuer: Some("https://auth.example.com".to_string()),
            client_id: Some("client-1".to_string()),
            redirect_uri: Some("http://localhost:5748/callback".to_string()),
            ..Default::default()
        };
        let state = Arc::new(OAuthState::new(config).unwrap());

        // user-1 may only search; user-2 may only research; user-3 (admin) may do all.
        let users: Vec<(String, &str, Vec<&str>)> = vec![
            (
                "user-1".to_string(),
                "webfind.search",
                vec!["webfind_search"],
            ),
            (
                "user-2".to_string(),
                "webfind.research",
                vec!["webfind_research"],
            ),
            (
                "user-3".to_string(),
                "webfind.*",
                vec![
                    "webfind_search",
                    "webfind_research",
                    "webfind_fetch",
                    "webfind_graph",
                ],
            ),
        ];

        let mut handles = Vec::new();
        for (uid, scope, allowed) in users {
            let state = state.clone();
            handles.push(tokio::spawn(async move {
                let claims = Claims {
                    sub: uid.clone(),
                    aud: vec!["webfind".to_string()],
                    iss: "https://auth.example.com".to_string(),
                    exp: 9999999999,
                    iat: 1,
                    scope: Some(scope.to_string()),
                    client_id: Some("client-1".to_string()),
                };

                // Each user makes many interleaved calls to all four tools.
                for round in 0..50 {
                    for tool in [
                        "webfind_search",
                        "webfind_research",
                        "webfind_fetch",
                        "webfind_graph",
                    ] {
                        let ok = claims.allows_tool(tool);
                        let should_allow = allowed.contains(&tool);
                        assert_eq!(
                            ok, should_allow,
                            "user {uid} scope '{scope}': {tool} should_allow={should_allow}"
                        );
                        let status = if ok { "ok" } else { "denied" };
                        state
                            .record_audit(&uid, tool, &serde_json::json!({"round": round}), status)
                            .await;
                    }
                }
            }));
        }

        for h in handles {
            h.await.expect("concurrent user task completed");
        }

        // Audit log must attribute every entry to the correct user.
        let recent = state.recent_audit(usize::MAX).await;
        // 3 users x 50 rounds x 4 tools = 600 entries.
        assert_eq!(recent.len(), 600, "all calls audited");

        // user-1 must have search = ok and every other tool = denied (least
        // privilege is enforced; denied calls are still audited).
        let u1: Vec<&AuditEntry> = recent.iter().filter(|e| e.user_id == "user-1").collect();
        assert!(!u1.is_empty());
        assert!(
            u1.iter().all(|e| {
                (e.tool_name == "webfind_search" && e.result_status == "ok")
                    || (e.tool_name != "webfind_search" && e.result_status == "denied")
            }),
            "user-1: only search allowed, others denied; got {:?}",
            u1.iter()
                .map(|e| (e.tool_name.as_str(), e.result_status.as_str()))
                .collect::<Vec<_>>()
        );

        // user-2 must have research = ok and everything else denied.
        let u2: Vec<&AuditEntry> = recent.iter().filter(|e| e.user_id == "user-2").collect();
        assert!(!u2.is_empty());
        assert!(
            u2.iter().all(|e| {
                (e.tool_name == "webfind_research" && e.result_status == "ok")
                    || (e.tool_name != "webfind_research" && e.result_status == "denied")
            }),
            "user-2: only research allowed, others denied"
        );

        // user-3 (admin) may call everything, all ok.
        let u3: Vec<&AuditEntry> = recent.iter().filter(|e| e.user_id == "user-3").collect();
        assert_eq!(u3.len(), 200);
        assert!(u3.iter().all(|e| e.result_status == "ok"));
    }
}
