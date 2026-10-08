//! Debug-only, opt-in browser access to the actual native application's dispatcher.
//! It is not a replacement engine or mock backend. Normal builds have no listener.
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use freesync_core::{Error, ErrorCode, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tauri::Manager;
#[derive(Clone)]
struct Bridge {
    app: tauri::AppHandle,
    nonce: String,
    origin: String,
    dist: PathBuf,
}
fn authorized(headers: &HeaderMap, nonce: &str, origin: &str, mutation: bool) -> bool {
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|cookies| {
            cookies
                .split(';')
                .any(|c| c.trim() == format!("FSUI={nonce}"))
        });
    cookie
        && (!mutation
            || headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) == Some(origin)
                && headers.get("x-freesync").and_then(|h| h.to_str().ok()) == Some("1"))
}
fn protected(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response.headers_mut().insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'none'; frame-ancestors 'none'".parse().unwrap());
    response
}
async fn launch(State(bridge): State<Bridge>, Path(nonce): Path<String>) -> Response {
    if nonce != bridge.nonce {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut response = StatusCode::SEE_OTHER.into_response();
    response
        .headers_mut()
        .insert(header::LOCATION, "/".parse().unwrap());
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("FSUI={}; HttpOnly; SameSite=Strict; Path=/", bridge.nonce)
            .parse()
            .unwrap(),
    );
    protected(response)
}
async fn asset(bridge: Bridge, headers: HeaderMap, path: &str) -> Response {
    if !authorized(&headers, &bridge.nonce, &bridge.origin, false) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path.contains("..")
        || path.contains('\\')
        || path.starts_with('/')
        || (!path.starts_with("assets/") && path != "index.html")
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let content = match tokio::fs::read(bridge.dist.join(path)).await {
        Ok(content) => content,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let content_type = if path.ends_with(".js") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    protected(
        Response::builder()
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(content))
            .unwrap(),
    )
}
async fn index(State(bridge): State<Bridge>, headers: HeaderMap) -> Response {
    asset(bridge, headers, "index.html").await
}
async fn files(
    State(bridge): State<Bridge>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    asset(bridge, headers, &path).await
}
#[derive(Deserialize)]
struct Request {
    command: String,
    args: Value,
}
async fn command(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Response {
    if !authorized(&headers, &bridge.nonce, &bridge.origin, true) {
        return StatusCode::FORBIDDEN.into_response();
    }
    protected(
        match crate::dispatch(bridge.app, &request.command, request.args).await {
            Ok(value) => Json(json!({"ok":value})).into_response(),
            Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error":error}))).into_response(),
        },
    )
}
pub fn start(app: tauri::AppHandle) -> Result<()> {
    if !cfg!(debug_assertions) {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "Browser QA is available only in debug builds.",
        ));
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let directory = app.state::<Arc<crate::backend::AppState>>().profile.clone();
    let data = json!({"url":format!("{origin}/launch/{nonce}"),"process":std::process::id(),"debug_only":true});
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(directory.join("browser-test.json"))?
        .write_all(serde_json::to_string(&data)?.as_bytes())?;
    let bridge = Bridge {
        app,
        nonce,
        origin,
        dist: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist"),
    };
    tauri::async_runtime::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let router = Router::new()
            .route("/launch/{nonce}", get(launch))
            .route("/", get(index))
            .route("/api/command", post(command))
            .route("/{*path}", get(files))
            .layer(DefaultBodyLimit::max(64 * 1024))
            .with_state(bridge);
        let _ = axum::serve(listener, router).await;
    });
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_capability_and_cross_origin_mutations() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "FSUI=private-session".parse().unwrap());
        assert!(authorized(
            &headers,
            "private-session",
            "http://127.0.0.1:1234",
            false
        ));
        assert!(!authorized(
            &headers,
            "wrong",
            "http://127.0.0.1:1234",
            false
        ));
        assert!(!authorized(
            &headers,
            "private-session",
            "http://127.0.0.1:1234",
            true
        ));
        headers.insert(header::ORIGIN, "https://unrelated.example".parse().unwrap());
        headers.insert("x-freesync", "1".parse().unwrap());
        assert!(!authorized(
            &headers,
            "private-session",
            "http://127.0.0.1:1234",
            true
        ));
        headers.insert(header::ORIGIN, "http://127.0.0.1:1234".parse().unwrap());
        assert!(authorized(
            &headers,
            "private-session",
            "http://127.0.0.1:1234",
            true
        ));
    }
}
