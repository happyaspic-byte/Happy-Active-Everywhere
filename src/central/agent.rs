use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpStream, time::timeout};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    invitation: Invitation,
    credential: String,
    name: String,
    device: String,
}

async fn exchange(state: &Path, config: &Config, request: Request) -> Result<Reply> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(
        config.invitation.certificate.clone(),
    ))?;
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_client_auth_cert(
        vec![rustls::pki_types::CertificateDer::from(read_private(
            &state.join("identity.der"),
        )?)],
        rustls::pki_types::PrivateKeyDer::Pkcs8(
            read_private(&state.join("identity.key.der"))?.into(),
        ),
    )?;
    tls.alpn_protocols = vec![PROTOCOL.to_vec()];
    tls.resumption = rustls::client::Resumption::disabled();
    let tcp = timeout(timing().io, TcpStream::connect(config.invitation.address)).await??;
    let mut stream = timeout(
        timing().io,
        tokio_rustls::TlsConnector::from(Arc::new(tls)).connect(
            rustls::pki_types::ServerName::try_from("everywhere.local")?,
            tcp,
        ),
    )
    .await??;
    let session = stream.get_ref().1;
    ensure!(
        session.alpn_protocol() == Some(PROTOCOL),
        "unexpected central protocol"
    );
    ensure!(
        session
            .peer_certificates()
            .and_then(|c| c.first())
            .is_some_and(|c| c.as_ref() == config.invitation.certificate),
        "central certificate changed"
    );
    crate::wire::send(&mut stream, request, timing()).await?;
    let reply: Reply = crate::wire::read(&mut stream, timing()).await?;
    if let Some(error) = &reply.error {
        anyhow::bail!("{error}");
    }
    Ok(reply)
}
fn load(state: &Path) -> Result<Config> {
    let config: Config = serde_json::from_slice(&read_private(&state.join("central-agent.json"))?)?;
    identity::valid_peer(&config.credential)?;
    identity::valid_peer(&config.invitation.token)?;
    ensure!(
        config.invitation.format == 1 && config.invitation.certificate.len() <= 8192,
        "unsupported invitation"
    );
    ensure!(
        identity::fingerprint(&read_private(&state.join("identity.der"))?) == config.device,
        "agent identity changed; enroll the recovered device separately"
    );
    Ok(config)
}
pub async fn enroll(state: &Path, invitation: &Path, name: &str) -> Result<Value> {
    let invitation: Invitation = serde_json::from_slice(&read_private(invitation)?)?;
    ensure!(
        invitation.format == 1 && invitation.certificate.len() <= 8192,
        "unsupported invitation"
    );
    identity::valid_peer(&invitation.token)?;
    ensure!(
        !name.trim().is_empty() && name.len() <= 120,
        "device name required (120 bytes maximum)"
    );
    if !state.try_exists()? {
        identity::init(state)?;
    }
    ensure!(
        !state.join("central-server.json").try_exists()?,
        "server state cannot also be an agent"
    );
    let _guard = crate::device::config_guard(state)?;
    let path = state.join("central-agent.json");
    if !path.try_exists()? {
        ensure!(
            invitation.expires >= now(),
            "invitation expired; create a new invitation in the console"
        );
        let device = identity::fingerprint(&read_private(&state.join("identity.der"))?);
        write_private(
            &path,
            &Config {
                invitation: invitation.clone(),
                credential: random_id()?,
                name: name.into(),
                device,
            },
        )?;
    }
    let config = load(state)?;
    ensure!(
        serde_json::to_value(&config.invitation)? == serde_json::to_value(invitation)?,
        "already configured for a different invitation; existing credentials are preserved"
    );
    drop(_guard);
    register(state, &config).await?;
    Ok(
        json!({"device":config.device,"central":config.invitation.address,"state":"pending-approval","next":"Approve this fingerprint in the central console, then start manage or the installed service."}),
    )
}
async fn register(state: &Path, config: &Config) -> Result<()> {
    exchange(
        state,
        config,
        Request::Enroll {
            invitation: config.invitation.token.clone(),
            credential: config.credential.clone(),
            name: config.name.clone(),
            certificate: read_private(&state.join("identity.der"))?,
        },
    )
    .await?;
    Ok(())
}
type Worker = tokio::task::JoinHandle<Result<Value>>;
// A large catalog must never prevent command outcomes or a remote pause from
// traversing the control channel. Keep the complete (bounded) job list first.
fn bounded_report(mut report: Value) -> Result<Value> {
    let total = report["folders"].as_array().map_or(0, Vec::len);
    report["folders_total"] = json!(total);
    while serde_json::to_vec(&report)?.len() > 512 * 1024 {
        report["report_truncated"] = json!(true);
        if let Some(folders) = report["folders"].as_array_mut().filter(|v| !v.is_empty()) {
            folders.truncate(folders.len() / 2);
        } else {
            return Ok(
                json!({"error":"Status summary exceeds the central limit. Control commands remain available; inspect detailed status on the device.","report_truncated":true,"folders_total":total}),
            );
        }
    }
    Ok(report)
}
struct Journal {
    db: Connection,
}
impl Journal {
    fn open(state: &Path) -> Result<Self> {
        let path = state.join("central-agent.sqlite");
        if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "unsafe agent journal"
            );
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS executed(id TEXT PRIMARY KEY,body TEXT NOT NULL,state TEXT NOT NULL,result TEXT,ack INTEGER NOT NULL DEFAULT 0);")?;
        let mut journal = Self { db };
        let interrupted = {
            let mut statement = journal
                .db
                .prepare("SELECT id FROM executed WHERE state='executing'")?;
            statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in interrupted {
            journal.finish(CommandResult { id, status: ResultState::Uncertain, value: json!({"error":"Agent stopped during execution. Inspect the device and file history before submitting a new command. This command was not replayed."}) })?;
        }
        Ok(journal)
    }
    fn finish(&mut self, mut result: CommandResult) -> Result<()> {
        if serde_json::to_vec(&result.value)?.len() > 512 * 1024 {
            result.status = ResultState::Failed;
            result.value = json!({"error":"Command output exceeds the central response limit; inspect it on the local device."});
        }
        self.db.execute(
            "UPDATE executed SET state='done',result=?1,ack=0 WHERE id=?2",
            params![serde_json::to_string(&result)?, result.id],
        )?;
        Ok(())
    }
    fn results(&self) -> Result<Vec<CommandResult>> {
        let mut statement = self
            .db
            .prepare("SELECT result FROM executed WHERE state='done' AND ack=0 LIMIT 1")?;
        statement
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
    fn start(&self, command: &Envelope) -> Result<bool> {
        identity::valid_peer(&command.id)?;
        let body = serde_json::to_string(&command.operation)?;
        let existing: Option<String> = self
            .db
            .query_row(
                "SELECT body FROM executed WHERE id=?1",
                [&command.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure!(
                existing == body,
                "central reused a command ID with different content"
            );
            self.db
                .execute("UPDATE executed SET ack=0 WHERE id=?1", [&command.id])?;
            return Ok(false);
        }
        self.db.execute(
            "INSERT INTO executed(id,body,state) VALUES(?1,?2,'executing')",
            params![command.id, body],
        )?;
        Ok(true)
    }
}
pub(crate) async fn run(
    state: PathBuf,
    jobs: Arc<crate::jobs::Jobs>,
    status: Arc<Mutex<Value>>,
) -> Result<()> {
    let config = load(&state)?;
    let mut journal = Journal::open(&state)?;
    let mut worker: Option<(String, Worker)> = None;
    let mut report = json!({});
    let mut next_report = 0;
    let mut registered = false;
    loop {
        if worker
            .as_ref()
            .is_some_and(|(_, handle)| handle.is_finished())
        {
            let (id, handle) = worker.take().unwrap();
            let (result_state, value) = match handle.await {
                Ok(Ok(value)) => (ResultState::Completed, value),
                Ok(Err(error)) => (ResultState::Failed, json!({"error":format!("{error:#}")})),
                Err(_) => (
                    ResultState::Uncertain,
                    json!({"error":"Command worker stopped unexpectedly; inspect the device before retrying."}),
                ),
            };
            journal.finish(CommandResult {
                id,
                status: result_state,
                value,
            })?;
            next_report = 0;
        }
        if now() >= next_report {
            let state = state.clone();
            report = tokio::task::spawn_blocking(move || crate::share::catalog(&state))
                .await?
                .unwrap_or_else(|error| json!({"error":format!("{error:#}")}));
            report["os"] = json!(std::env::consts::OS);
            next_report = now() + 30;
        }
        report["jobs"] = jobs.status()?;
        let request = Request::Poll {
            device: config.device.clone(),
            credential: config.credential.clone(),
            report: bounded_report(report.clone())?,
            results: journal.results()?,
            active: worker.as_ref().map(|(id, _)| id.clone()),
        };
        let response = async {
            if !registered {
                register(&state, &config).await?;
                registered = true;
            }
            exchange(&state, &config, request).await
        }
        .await;
        match response {
            Ok(reply) => {
                *status
                    .lock()
                    .map_err(|_| anyhow::anyhow!("agent status unavailable"))? = json!({"state":reply.state,"last_contact":now(),"address":config.invitation.address});
                for id in reply.acknowledged {
                    journal.db.execute(
                        "UPDATE executed SET ack=1 WHERE id=?1 AND state='done'",
                        [id],
                    )?;
                }
                if let Some(command) = reply.command {
                    ensure!(
                        reply.state == "approved" && worker.is_none(),
                        "unexpected central command"
                    );
                    if journal.start(&command)? {
                        let state = state.clone();
                        let jobs = jobs.clone();
                        worker = Some((
                            command.id,
                            tokio::task::spawn_blocking(move || {
                                crate::management::execute(&state, &jobs, command.operation)
                            }),
                        ));
                    }
                }
            }
            Err(error) => {
                *status
                    .lock()
                    .map_err(|_| anyhow::anyhow!("agent status unavailable"))? = json!({"state":"disconnected","error":format!("{error:#}"),"address":config.invitation.address});
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
