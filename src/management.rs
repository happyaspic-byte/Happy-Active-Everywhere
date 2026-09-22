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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    device: String,
    token: String,
    host: String,
    jobs: Arc<crate::jobs::Jobs>,
    central: Arc<Mutex<Value>>,
}
async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!({"status":"ok", "device":app.device,"pid":std::process::id()}))
}
pub(crate) fn equal_secret(left: &str, right: &str) -> bool {
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
        match tokio::task::spawn_blocking(move || -> Result<Value> {
            let mut status = share::catalog(&app.state)?;
            status["jobs"] = app.jobs.status()?;
            status["central"] = app
                .central
                .lock()
                .map_err(|_| anyhow::anyhow!("agent status unavailable"))?
                .clone();
            Ok(status)
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        },
    )
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Operation {
    Deploy {
        root: PathBuf,
        mode: Mode,
        certificate: Vec<u8>,
        job: crate::jobs::Config,
    },
    TrustPeer {
        certificate: Vec<u8>,
        expected: String,
    },
    SaveJob {
        job: crate::jobs::Config,
    },
    SetJobEnabled {
        id: String,
        enabled: bool,
    },
    Pending {
        folder: String,
    },
    ApproveDeletion {
        folder: String,
        path: String,
        expected: crate::model::Versions,
    },
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
pub(crate) fn execute(
    state: &Path,
    jobs: &crate::jobs::Jobs,
    operation: Operation,
) -> Result<Value> {
    match operation {
        Operation::Deploy {
            root,
            mode,
            certificate,
            job,
        } => {
            job.validate()?;
            ensure!(
                identity::fingerprint(&certificate) == job.peer,
                "peer certificate mismatch"
            );
            jobs.validate_deployment(&job)?;
            share::ensure_registration(state, &job.folder, &root, mode)?;
            execute(
                state,
                jobs,
                Operation::TrustPeer {
                    certificate,
                    expected: job.peer.clone(),
                },
            )?;
            share::grant(state, &job.folder, &job.peer, false)?;
            jobs.save_deployment(job)?;
        }
        Operation::TrustPeer {
            certificate,
            expected,
        } => {
            identity::valid_peer(&expected)?;
            ensure!(
                certificate.len() <= 64 * 1024 && identity::fingerprint(&certificate) == expected,
                "certificate fingerprint does not match the independently verified device"
            );
            let trusted = state.join("peers").join(format!("{expected}.der"));
            if trusted.try_exists()? {
                ensure!(
                    fs::symlink_metadata(&trusted)?.file_type().is_file()
                        && fs::read(trusted)? == certificate,
                    "existing trust does not match"
                );
                return Ok(json!({"peer":expected}));
            }
            let temporary = state.join(format!("peer-import-{}.der", random_id()?));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&certificate)?;
            file.sync_all()?;
            drop(file);
            let result = identity::trust(state, &temporary);
            let _ = fs::remove_file(temporary);
            return Ok(json!({"peer":result?}));
        }
        Operation::SaveJob { job } => {
            jobs.save(job)?;
        }
        Operation::SetJobEnabled { id, enabled } => {
            jobs.set_enabled(&id, enabled)?;
        }
        Operation::Pending { folder } => {
            return Share::open(state, &folder)?.pending();
        }
        Operation::ApproveDeletion {
            folder,
            path,
            expected,
        } => {
            Share::open(state, &folder)?.approve_deletion(&path, &expected)?;
        }
        Operation::CreateFolder { folder, root, mode } => {
            Share::create(state, &folder, &root, mode)?;
        }
        Operation::Scan { folder } => {
            return Ok(serde_json::to_value(
                Share::open(state, &folder)?.scan(false)?,
            )?);
        }
        Operation::Conflicts { folder } => {
            return Share::open(state, &folder)?.conflicts();
        }
        Operation::History { folder, path } => {
            return Share::open(state, &folder)?.history(&path);
        }
        Operation::Resolve {
            folder,
            path,
            revision,
        } => {
            Share::open(state, &folder)?.choose(&path, &revision, false)?;
        }
        Operation::Restore {
            folder,
            path,
            revision,
        } => {
            Share::open(state, &folder)?.choose(&path, &revision, true)?;
        }
        Operation::Grant {
            folder,
            peer,
            remove,
        } => {
            share::grant(state, &folder, &peer, remove)?;
        }
    }
    Ok(json!({"ok":true}))
}
async fn command(State(app): State<Arc<App>>, Json(operation): Json<Operation>) -> Response {
    outcome(
        match tokio::task::spawn_blocking(move || -> Result<Value> {
            execute(&app.state, &app.jobs, operation)
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
    if state.join("central-server.json").try_exists()? {
        return crate::central::serve(&state, listen).await;
    }
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
    let jobs = Arc::new(crate::jobs::Jobs::open(&state)?);
    let supervisor_jobs = jobs.clone();
    let supervisor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            interval.tick().await;
            let jobs = supervisor_jobs.clone();
            let _ = tokio::task::spawn_blocking(move || jobs.tick()).await;
        }
    });
    let central = Arc::new(Mutex::new(json!({"state":"not-enrolled"})));
    let agent = if state.join("central-agent.json").try_exists()? {
        let state = state.clone();
        let jobs = jobs.clone();
        let status = central.clone();
        Some(tokio::spawn(async move {
            if let Err(error) = crate::central::run_agent(state, jobs, status.clone()).await {
                if let Ok(mut value) = status.lock() {
                    *value = json!({"state":"error","error":format!("{error:#}")});
                }
                eprintln!("Central agent stopped: {error:#}");
            }
        }))
    } else {
        None
    };
    let app = Arc::new(App {
        central,
        device: identity::fingerprint(&fs::read(state.join("identity.der"))?),
        state,
        token: credential,
        host: address.to_string(),
        jobs: jobs.clone(),
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
        .route("/api/health", get(health))
        .route("/api/command", post(command))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    println!("LISTEN {address}");
    std::io::stdout().flush()?;
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    if let Some(agent) = agent {
        agent.abort();
    }
    supervisor.abort();
    jobs.shutdown();
    result?;
    Ok(())
}
