//! Loopback-only management API. The bearer credential is never embedded in assets.
use crate::{
    identity,
    root::random_id,
    share::{self, Share},
    versions::Mode,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn token(state: &Path) -> Result<String> {
    ensure!(
        state.join("identity.der").is_file(),
        "initialize the device first"
    );
    let path = state.join("management.token");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            let value = random_id()?;
            file.write_all(value.as_bytes())?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_file(),
        "unsafe management credential"
    );
    let mut value = String::new();
    File::open(path)?.take(65).read_to_string(&mut value)?;
    identity::valid_peer(&value)?;
    Ok(value)
}
struct App {
    state: PathBuf,
    token: String,
    host: String,
}
fn equal_secret(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |a, (x, y)| a | (x ^ y))
            == 0
}
async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host_ok =
        headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(app.host.as_str());
    let origin_ok = headers.get(header::ORIGIN).is_none_or(|v| {
        v.to_str()
            .is_ok_and(|v| v == format!("http://{}", app.host))
    });
    let site_ok = headers
        .get("sec-fetch-site")
        .is_none_or(|v| v != "cross-site");
    let mut response = if !host_ok || !origin_ok || !site_ok {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"untrusted request origin or host"})),
        )
            .into_response()
    } else if request.uri().path().starts_with("/api/")
        && !headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|v| equal_secret(v, &app.token))
    {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"management token required"})),
        )
            .into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
fn outcome(result: Result<Value>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error":format!("{error:#}")})),
        )
            .into_response(),
    }
}
async fn status(State(app): State<Arc<App>>) -> Response {
    outcome(
        match tokio::task::spawn_blocking(move || share::catalog(&app.state)).await {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        },
    )
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Operation {
    Pending { folder: String },
    ApproveDeletion { folder: String, path: String, expected: crate::model::Versions },
    CreateFolder {
        folder: String,
        root: PathBuf,
        mode: Mode,
    },
    Scan {
        folder: String,
    },
    Conflicts {
        folder: String,
    },
    History {
        folder: String,
        path: String,
    },
    Resolve {
        folder: String,
        path: String,
        revision: String,
    },
    Restore {
        folder: String,
        path: String,
        revision: String,
    },
    Grant {
        folder: String,
        peer: String,
        remove: bool,
    },
}
async fn command(State(app): State<Arc<App>>, Json(operation): Json<Operation>) -> Response {
    outcome(
        match tokio::task::spawn_blocking(move || -> Result<Value> {
            match operation {
            Operation::Pending { folder } => { return Share::open(&app.state, &folder)?.pending(); }
            Operation::ApproveDeletion { folder, path, expected } => { Share::open(&app.state, &folder)?.approve_deletion(&path, &expected)?; }
                Operation::CreateFolder { folder, root, mode } => {
                    Share::create(&app.state, &folder, &root, mode)?;
                }
                Operation::Scan { folder } => {
                    return Ok(serde_json::to_value(
                        Share::open(&app.state, &folder)?.scan(false)?,
                    )?);
                }
                Operation::Conflicts { folder } => {
                    return Share::open(&app.state, &folder)?.conflicts();
                }
                Operation::History { folder, path } => {
                    return Share::open(&app.state, &folder)?.history(&path);
                }
                Operation::Resolve {
                    folder,
                    path,
                    revision,
                } => {
                    Share::open(&app.state, &folder)?.choose(&path, &revision, false)?;
                }
                Operation::Restore {
                    folder,
                    path,
                    revision,
                } => {
                    Share::open(&app.state, &folder)?.choose(&path, &revision, true)?;
                }
                Operation::Grant {
                    folder,
                    peer,
                    remove,
                } => {
                    share::grant(&app.state, &folder, &peer, remove)?;
                }
            }
            Ok(json!({"ok":true}))
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        },
    )
}
pub async fn serve(state: &Path, listen: SocketAddr) -> Result<()> {
    ensure!(
        listen.ip().is_loopback(),
        "management must listen on a loopback address"
    );
    let state = state.canonicalize()?;
    let credential = token(&state)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("management.lock"))?;
    lock.try_lock_exclusive()
        .context("management is already running")?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let address = listener.local_addr()?;
    let app = Arc::new(App {
        state,
        token: credential,
        host: address.to_string(),
    });
    let router = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../ui/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    Body::from(include_str!("../ui/app.js")),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    Body::from(include_str!("../ui/app.css")),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/command", post(command))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    println!("LISTEN {address}");
    std::io::stdout().flush()?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
