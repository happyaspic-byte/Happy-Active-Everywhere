use super::store::Store;
use super::*;
use anyhow::Context;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path as UrlPath, Request as HttpRequest, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use fs2::FileExt;
use serde_json::json;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, sync::Semaphore, time::timeout};

struct App {
    db: Arc<Mutex<Store>>,
    token: String,
    host: String,
    config: ServerConfig,
    certificate: Vec<u8>,
    device: String,
}
async fn guard(State(app): State<Arc<App>>, request: HttpRequest, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let expected_origin = if host == app.host {
        Some(format!("http://{host}"))
    } else {
        app.config
            .public_origin
            .as_ref()
            .filter(|origin| origin.strip_prefix("https://") == Some(host))
            .cloned()
    };
    let trusted = expected_origin.as_ref().is_some_and(|origin| {
        headers
            .get(header::ORIGIN)
            .is_none_or(|v| v.to_str().ok() == Some(origin))
    }) && headers
        .get("sec-fetch-site")
        .is_none_or(|v| v != "cross-site");
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| crate::management::equal_secret(v, &app.token));
    let mut response = if !trusted {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"untrusted request origin or host"})),
        )
            .into_response()
    } else if request.uri().path().starts_with("/api/") && !authorized {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"central management token required"})),
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
async fn database(
    app: Arc<App>,
    action: impl FnOnce(&mut Store) -> Result<Value> + Send + 'static,
) -> Response {
    outcome(
        match tokio::task::spawn_blocking(move || {
            action(
                &mut *app
                    .db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("central registry unavailable"))?,
            )
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        },
    )
}
async fn status(State(app): State<Arc<App>>) -> Response {
    let control = app.config.control_listen;
    database(app, move |db| {
        let mut status = db.status()?;
        status["control_listen"] = json!(control);
        Ok(status)
    })
    .await
}
async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!({"status":"ok","device":app.device,"pid":std::process::id(),"role":"central"}))
}
async fn command_status(State(app): State<Arc<App>>, UrlPath(id): UrlPath<String>) -> Response {
    database(app, move |db| db.command(&id)).await
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Action {
    Invite {
        name: String,
        address: SocketAddr,
    },
    SetDevice {
        device: String,
        approved: bool,
    },
    Command {
        device: String,
        operation: Operation,
    },
    Cancel {
        id: String,
    },
    Pair {
        folder: String,
        listener: String,
        connector: String,
        listener_root: PathBuf,
        connector_root: PathBuf,
        address: SocketAddr,
        listener_mode: crate::versions::Mode,
        connector_mode: crate::versions::Mode,
    },
}
async fn action(State(app): State<Arc<App>>, Json(action): Json<Action>) -> Response {
    let certificate = app.certificate.clone();
    database(app, move |db| {
        match action {
            Action::Invite { name,address } => Ok(serde_json::to_value(db.invite(&name,address,certificate)?)?),
            Action::SetDevice { device,approved } => { db.set_device(&device,approved)?; Ok(json!({"ok":true})) },
            Action::Cancel { id } => { db.cancel(&id)?; Ok(json!({"ok":true})) },
            Action::Command { device,operation } => Ok(json!({"commands":db.queue(vec![(device,operation)])?})),
            Action::Pair { folder,listener,connector,listener_root,connector_root,address,listener_mode,connector_mode } => {
                ensure!(listener != connector, "select two different devices");
                ensure!(!address.ip().is_unspecified() && !address.ip().is_multicast() && address.port() != 0, "enter the listener's reachable IP and port");
                ensure!(listener_mode != crate::versions::Mode::SendOnly || connector_mode != crate::versions::Mode::SendOnly, "both devices cannot be send-only");
                ensure!(listener_mode != crate::versions::Mode::ReceiveOnly || connector_mode != crate::versions::Mode::ReceiveOnly, "both devices cannot be receive-only");
                let listener_cert = db.certificate(&listener)?;
                let connector_cert = db.certificate(&connector)?;
                let listen_ip = match address.ip() { IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED), IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED) };
                let job = |peer: &str, direction, address| crate::jobs::Config {
                    id: format!("central-{}", &secret_hash(&format!("{folder}:{peer}"))[..32]),
                    folder: folder.clone(), peer: peer.into(), direction, address, enabled: true,
                };
                let listener_job = job(&connector, crate::jobs::Direction::Listen, SocketAddr::new(listen_ip,address.port()));
                let connector_job = job(&listener, crate::jobs::Direction::Connect,address);
                listener_job.validate()?; connector_job.validate()?;
                Ok(json!({"commands":db.queue(vec![
                    (listener, Operation::Deploy { root:listener_root,mode:listener_mode,certificate:connector_cert,job:listener_job }),
                    (connector, Operation::Deploy { root:connector_root,mode:connector_mode,certificate:listener_cert,job:connector_job }),
                ])?}))
            }
        }
    }).await
}
async fn control(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    db: Arc<Mutex<Store>>,
) -> Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let slots = Arc::new(Semaphore::new(64));
    loop {
        let (tcp, _) = listener.accept().await?;
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            drop(tcp);
            continue;
        };
        let acceptor = acceptor.clone();
        let db = db.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let _ = timeout(Duration::from_secs(40), async move {
                let mut stream = timeout(timing().io, acceptor.accept(tcp)).await??;
                ensure!(
                    stream.get_ref().1.alpn_protocol() == Some(PROTOCOL),
                    "unexpected central protocol"
                );
                let certificate = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|c| c.first())
                    .context("device certificate required")?
                    .to_vec();
                let request: super::Request = crate::wire::read(&mut stream, timing()).await?;
                match &request {
                    super::Request::Enroll {
                        certificate: requested,
                        ..
                    } => ensure!(
                        requested == &certificate,
                        "enrollment certificate is not the TLS device"
                    ),
                    super::Request::Poll { device, .. } => ensure!(
                        device == &identity::fingerprint(&certificate),
                        "poll identity is not the TLS device"
                    ),
                }
                let result = tokio::task::spawn_blocking(move || {
                    db.lock()
                        .map_err(|_| anyhow::anyhow!("central registry unavailable"))?
                        .exchange(request)
                })
                .await?;
                let reply = result.unwrap_or_else(|error| Reply {
                    error: Some(format!("{error:#}")),
                    ..Reply::state("rejected")
                });
                crate::wire::send(&mut stream, reply, timing()).await
            })
            .await;
        });
    }
}
pub async fn serve(state: &Path, listen: SocketAddr) -> Result<()> {
    ensure!(
        listen.ip().is_loopback(),
        "central web must use loopback; use an HTTPS reverse proxy or SSH tunnel for remote browsers"
    );
    let config: ServerConfig =
        serde_json::from_slice(&read_private(&state.join("central-server.json"))?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("management.lock"))?;
    lock.try_lock_exclusive()
        .context("central management is already running")?;
    let certificate = read_private(&state.join("identity.der"))?;
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_client_cert_verifier(Arc::new(super::tls::EnrollmentVerifier))
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(certificate.clone())],
        rustls::pki_types::PrivateKeyDer::Pkcs8(
            read_private(&state.join("identity.key.der"))?.into(),
        ),
    )?;
    tls.alpn_protocols = vec![PROTOCOL.to_vec()];
    tls.send_tls13_tickets = 0;
    let db = Arc::new(Mutex::new(Store::open(state)?));
    let control_listener = TcpListener::bind(config.control_listen).await?;
    let listener = TcpListener::bind(listen).await?;
    let address = listener.local_addr()?;
    let control_address = control_listener.local_addr()?;
    let app = Arc::new(App {
        db: db.clone(),
        token: crate::management::token(state)?,
        host: address.to_string(),
        device: identity::fingerprint(&certificate),
        certificate,
        config,
    });
    let router = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../ui/central.html")) }),
        )
        .route(
            "/central.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    Body::from(include_str!("../../ui/central.js")),
                )
            }),
        )
        .route(
            "/central.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    Body::from(include_str!("../../ui/central.css")),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/health", get(health))
        .route("/api/command/{id}", get(command_status))
        .route("/api/action", post(action))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    let mut control_task = tokio::spawn(control(control_listener, Arc::new(tls), db));
    println!("LISTEN {address}\nCONTROL {control_address}");
    std::io::stdout().flush()?;
    let web = axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    });
    tokio::select! {
        result = web => { control_task.abort(); result?; }
        result = &mut control_task => { result??; anyhow::bail!("central control listener stopped"); }
    }
    Ok(())
}
