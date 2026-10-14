//! MCP Streamable HTTP transport for local clients.
//!
//! Listens on 127.0.0.1 only and requires `Authorization: Bearer <token>`
//! with the token from [`token_path`]. Host and Origin headers are checked
//! against loopback names to stop DNS rebinding from browsers. Every
//! `Mcp-Session-Id` gets its own server state; responses are plain JSON (no
//! server-initiated SSE stream).

use crate::indexer::Indexer;
use crate::mcp_server::McpServer;
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const DEFAULT_PORT: u16 = 7337;
pub const ENDPOINT: &str = "/mcp";
const SESSION_HEADER: &str = "mcp-session-id";
const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_SESSIONS: usize = 8;
const SESSION_IDLE: Duration = Duration::from_secs(60 * 60);

pub fn token_path() -> PathBuf {
    crate::workspace::base_dir().join("http-token")
}

/// Read the access token, creating a random one (mode 0600) on first use.
pub fn load_or_create_token() -> Result<String> {
    let path = token_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim().to_string();
        if token.len() >= 32 {
            restrict_to_owner(&path)
                .with_context(|| format!("failed to restrict {}", path.display()))?;
            return Ok(token);
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(&path, &token).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(token)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    restrict_to_owner(path)?;
    file.write_all(contents.as_bytes())
}

/// `mode` above only applies when the file is created; a token file that
/// already exists may have been written group- or world-readable.
#[cfg(unix)]
fn restrict_to_owner(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

struct Session {
    server: Mutex<McpServer>,
    last_used: std::sync::Mutex<Instant>,
}

struct State {
    token: String,
    port: u16,
    indexer: Arc<Mutex<Indexer>>,
    max_backups: usize,
    scope: Option<String>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

pub async fn serve(port: u16, max_backups: usize, scope: Option<String>) -> Result<()> {
    crate::update_check::record_startup_fingerprint();
    let token = load_or_create_token()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(State {
        token,
        port,
        indexer: McpServer::shared_indexer()?,
        max_backups,
        scope,
        sessions: Mutex::new(HashMap::new()),
    });
    eprintln!("RMS Memory MCP (Streamable HTTP) on http://127.0.0.1:{port}{ENDPOINT}");
    eprintln!(
        "Clients must send `Authorization: Bearer <token>`; the token is in {}",
        token_path().display()
    );

    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = tokio::signal::ctrl_c() => break,
        };
        let state = state.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(handle(&state, request).await) }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await
            {
                tracing::debug!("HTTP connection ended: {error}");
            }
        });
    }
    for session in state.sessions.lock().await.values() {
        session.server.lock().await.shutdown();
    }
    Ok(())
}

fn respond(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response
}

fn json_response(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    let mut response = respond(status, serde_json::to_vec(value).unwrap_or_default());
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn loopback_host(host: &str) -> &str {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    host.rsplit_once(':').map(|(name, _)| name).unwrap_or(host)
}

fn is_loopback_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// Host must name this loopback listener; a present Origin must be loopback.
fn check_headers(headers: &HeaderMap, port: u16) -> Result<(), &'static str> {
    let host = headers
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or("missing Host header")?;
    let port_matches = host
        .rsplit_once(':')
        .is_some_and(|(_, value)| value == port.to_string());
    if !is_loopback_name(loopback_host(host)) || !port_matches {
        return Err("Host must be a loopback address of this server");
    }
    if let Some(origin) = headers.get(hyper::header::ORIGIN) {
        let origin = origin.to_str().map_err(|_| "invalid Origin header")?;
        let authority = origin
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        if !is_loopback_name(loopback_host(authority)) {
            return Err("cross-origin requests are not allowed");
        }
    }
    Ok(())
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(presented) = headers
        .get(hyper::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (presented.trim().as_bytes(), token.as_bytes());
    // Every byte is compared so the time does not reveal the matching prefix.
    let diff = a
        .iter()
        .zip(b)
        .fold(0u8, |acc, (x, y)| acc | std::hint::black_box(x ^ y));
    a.len() == b.len() && std::hint::black_box(diff) == 0
}

fn is_initialize(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("initialize")
}

async fn handle(state: &State, request: Request<Incoming>) -> Response<Full<Bytes>> {
    if request.uri().path() != ENDPOINT {
        return respond(StatusCode::NOT_FOUND, "Not found");
    }
    if let Err(reason) = check_headers(request.headers(), state.port) {
        return respond(StatusCode::FORBIDDEN, reason);
    }
    if !authorized(request.headers(), &state.token) {
        let mut response = respond(StatusCode::UNAUTHORIZED, "Missing or invalid bearer token");
        response.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer"),
        );
        return response;
    }
    let session_id = request
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    match *request.method() {
        Method::POST => post(state, session_id, request).await,
        Method::DELETE => {
            let removed = match &session_id {
                Some(id) => state.sessions.lock().await.remove(id),
                None => None,
            };
            match removed {
                Some(session) => {
                    session.server.lock().await.shutdown();
                    respond(StatusCode::OK, "")
                }
                None => respond(StatusCode::NOT_FOUND, "Unknown session"),
            }
        }
        _ => {
            let mut response = respond(StatusCode::METHOD_NOT_ALLOWED, "");
            response.headers_mut().insert(
                hyper::header::ALLOW,
                HeaderValue::from_static("POST, DELETE"),
            );
            response
        }
    }
}

async fn post(
    state: &State,
    session_id: Option<String>,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let body = match Limited::new(request.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &McpServer::parse_error("Request too large (max 1MB)".to_string()),
            );
        }
    };
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                &McpServer::parse_error(format!("Parse error: {error}")),
            );
        }
    };
    let batch = payload.is_array();
    let messages = match payload {
        Value::Array(messages) => messages,
        message => vec![message],
    };

    let (session, created) = match session_id {
        Some(id) => match state.sessions.lock().await.get(&id) {
            Some(session) => (session.clone(), None),
            None => return respond(StatusCode::NOT_FOUND, "Unknown or expired session"),
        },
        None if messages.iter().any(is_initialize) => {
            let id = uuid::Uuid::new_v4().to_string();
            let session = Arc::new(Session {
                server: Mutex::new(McpServer::new(
                    None,
                    state.indexer.clone(),
                    None,
                    state.max_backups,
                    state.scope.clone(),
                    true,
                )),
                last_used: std::sync::Mutex::new(Instant::now()),
            });
            admit(state, id.clone(), session.clone()).await;
            (session, Some(id))
        }
        None => {
            return respond(
                StatusCode::BAD_REQUEST,
                "Missing Mcp-Session-Id; start with an initialize request",
            );
        }
    };
    if let Ok(mut last_used) = session.last_used.lock() {
        *last_used = Instant::now();
    }

    let mut responses = Vec::new();
    {
        let mut server = session.server.lock().await;
        for message in messages {
            let reply: futures::future::BoxFuture<'_, Result<Option<Value>>> =
                Box::pin(server.handle_message(message));
            match reply.await {
                // Server-initiated requests need an SSE stream this transport
                // does not offer; only responses (which carry an id and no
                // method) go back to the client.
                Ok(Some(reply)) if reply.get("method").is_none() => responses.push(reply),
                Ok(_) => {}
                Err(error) => responses.push(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32600, "message": format!("Invalid request: {error}") }
                })),
            }
        }
    }

    let mut response = if responses.is_empty() {
        respond(StatusCode::ACCEPTED, "")
    } else if batch {
        json_response(StatusCode::OK, &Value::Array(responses))
    } else {
        json_response(StatusCode::OK, &responses.remove(0))
    };
    if let Some(id) = created
        && let Ok(value) = HeaderValue::from_str(&id)
    {
        response.headers_mut().insert(SESSION_HEADER, value);
    }
    response
}

/// Register a session, dropping idle ones and then the least recently used
/// when the cap is reached.
async fn admit(state: &State, id: String, session: Arc<Session>) {
    let mut sessions = state.sessions.lock().await;
    let idle_since = |session: &Session| {
        session
            .last_used
            .lock()
            .map(|last_used| *last_used)
            .unwrap_or_else(|_| Instant::now())
    };
    let mut evicted: Vec<Arc<Session>> = Vec::new();
    sessions.retain(|_, session| {
        let keep = idle_since(session).elapsed() < SESSION_IDLE;
        if !keep {
            evicted.push(session.clone());
        }
        keep
    });
    while sessions.len() >= MAX_SESSIONS {
        let Some(oldest) = sessions
            .iter()
            .min_by_key(|(_, session)| idle_since(session))
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        if let Some(session) = sessions.remove(&oldest) {
            evicted.push(session);
        }
    }
    sessions.insert(id, session);
    drop(sessions);
    for session in evicted {
        session.server.lock().await.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_loopback_host_and_origin_are_accepted() {
        assert!(check_headers(&headers(&[("host", "127.0.0.1:7337")]), 7337).is_ok());
        assert!(check_headers(&headers(&[("host", "localhost:7337")]), 7337).is_ok());
        assert!(check_headers(&headers(&[("host", "[::1]:7337")]), 7337).is_ok());
        assert!(
            check_headers(
                &headers(&[
                    ("host", "127.0.0.1:7337"),
                    ("origin", "http://localhost:3000")
                ]),
                7337
            )
            .is_ok()
        );
        for bad in [
            headers(&[]),
            headers(&[("host", "evil.example:7337")]),
            headers(&[("host", "127.0.0.1:8080")]),
            headers(&[("host", "127.0.0.1")]),
            headers(&[
                ("host", "127.0.0.1:7337"),
                ("origin", "https://evil.example"),
            ]),
            headers(&[("host", "127.0.0.1:7337"), ("origin", "null")]),
        ] {
            assert!(check_headers(&bad, 7337).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_world_readable_token_file_is_restricted_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("http-token");
        std::fs::write(&path, "t".repeat(64)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        restrict_to_owner(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        write_private(&path, &"u".repeat(64)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn bearer_token_must_match_exactly() {
        let token = "a".repeat(64);
        assert!(authorized(
            &headers(&[("authorization", &format!("Bearer {token}"))]),
            &token
        ));
        assert!(!authorized(&headers(&[]), &token));
        assert!(!authorized(
            &headers(&[("authorization", &format!("Bearer {}", "a".repeat(63)))]),
            &token
        ));
        assert!(!authorized(
            &headers(&[("authorization", &format!("Basic {token}"))]),
            &token
        ));
    }
}
