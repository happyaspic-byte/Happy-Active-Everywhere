use super::*;
use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

pub(super) struct Store {
    db: Connection,
}
impl Store {
    pub fn open(state: &Path) -> Result<Self> {
        let path = state.join("central.sqlite");
        if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "unsafe central database"
            );
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS invites(hash TEXT PRIMARY KEY,label TEXT NOT NULL,expires INTEGER NOT NULL,device TEXT);
            CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY,name TEXT NOT NULL,credential TEXT NOT NULL,certificate TEXT NOT NULL,state TEXT NOT NULL,seen INTEGER NOT NULL,report TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS commands(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,device TEXT NOT NULL,body TEXT NOT NULL,status TEXT NOT NULL,result TEXT,created INTEGER NOT NULL,updated INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS command_device ON commands(device,status,seq);
            CREATE TABLE IF NOT EXISTS audit(seq INTEGER PRIMARY KEY,action TEXT NOT NULL,device TEXT NOT NULL,created INTEGER NOT NULL);")?;
        Ok(Self { db })
    }
    pub fn invite(
        &self,
        label: &str,
        address: SocketAddr,
        certificate: Vec<u8>,
    ) -> Result<Invitation> {
        ensure!(
            !label.trim().is_empty() && label.len() <= 120,
            "invitation name required (120 bytes maximum)"
        );
        ensure!(
            address.port() != 0 && !address.ip().is_unspecified() && !address.ip().is_multicast(),
            "use an address reachable from the agent"
        );
        let token = random_id()?;
        let expires = now() + 24 * 60 * 60;
        self.db.execute(
            "INSERT INTO invites VALUES(?1,?2,?3,NULL)",
            params![secret_hash(&token), label, expires],
        )?;
        Ok(Invitation {
            format: 1,
            address,
            certificate,
            token,
            expires,
        })
    }
    pub fn exchange(&mut self, request: Request) -> Result<Reply> {
        match request {
            Request::Enroll {
                invitation,
                credential,
                name,
                certificate,
            } => {
                identity::valid_peer(&invitation)?;
                identity::valid_peer(&credential)?;
                ensure!(
                    !name.trim().is_empty()
                        && name.len() <= 120
                        && !name.chars().any(char::is_control),
                    "invalid device name"
                );
                ensure!(certificate.len() <= 8192, "certificate too large");
                rustls::RootCertStore::empty()
                    .add(rustls::pki_types::CertificateDer::from(certificate.clone()))?;
                let device = identity::fingerprint(&certificate);
                let tx = self.db.transaction()?;
                let (expires, used): (i64, Option<String>) = tx
                    .query_row(
                        "SELECT expires,device FROM invites WHERE hash=?1",
                        [secret_hash(&invitation)],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .context("invalid enrollment invitation")?;
                if let Some(used) = used {
                    let hash: Option<String> = tx
                        .query_row(
                            "SELECT credential FROM devices WHERE id=?1",
                            [&device],
                            |r| r.get(0),
                        )
                        .optional()?;
                    ensure!(
                        used == device && hash.as_deref() == Some(&secret_hash(&credential)),
                        "invitation already used"
                    );
                } else {
                    ensure!(expires >= now(), "enrollment invitation expired");
                    let count: i64 =
                        tx.query_row("SELECT count(*) FROM devices", [], |r| r.get(0))?;
                    ensure!(count < 128, "device limit reached");
                    tx.execute(
                        "INSERT INTO devices VALUES(?1,?2,?3,?4,'pending',?5,'{}')",
                        params![
                            device,
                            name,
                            secret_hash(&credential),
                            serde_json::to_string(&certificate)?,
                            now()
                        ],
                    )
                    .context("device already enrolled; use its existing agent configuration")?;
                    tx.execute(
                        "UPDATE invites SET device=?1 WHERE hash=?2",
                        params![device, secret_hash(&invitation)],
                    )?;
                }
                tx.commit()?;
                Ok(Reply::state("registered"))
            }
            Request::Poll {
                device,
                credential,
                mut report,
                results,
                active,
            } => {
                identity::valid_peer(&device)?;
                identity::valid_peer(&credential)?;
                ensure!(results.len() <= 16, "too many command results");
                if serde_json::to_vec(&report)?.len() > 768 * 1024 {
                    report = json!({"error":"Agent status summary exceeded the server limit. Command control remains available.","report_truncated":true});
                }
                let tx = self.db.transaction()?;
                let (hash, state): (String, String) = tx
                    .query_row(
                        "SELECT credential,state FROM devices WHERE id=?1",
                        [&device],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .context("agent is not enrolled")?;
                ensure!(
                    crate::management::equal_secret(&hash, &secret_hash(&credential)),
                    "agent authentication rejected"
                );
                let mut reply = Reply::state(&state);
                tx.execute(
                    "UPDATE devices SET seen=?1 WHERE id=?2",
                    params![now(), device],
                )?;
                if state == "approved" {
                    tx.execute(
                        "UPDATE devices SET report=?1 WHERE id=?2",
                        params![serde_json::to_string(&report)?, device],
                    )?;
                }
                for result in results {
                    let status = match result.status {
                        ResultState::Completed => "completed",
                        ResultState::Failed => "failed",
                        ResultState::Uncertain => "uncertain",
                    };
                    let changed = tx.execute("UPDATE commands SET status=?1,result=?2,updated=?3 WHERE id=?4 AND device=?5 AND status='delivered'", params![status,serde_json::to_string(&result.value)?,now(),result.id,device])?;
                    let terminal: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM commands WHERE id=?1 AND device=?2 AND status IN ('completed','failed','uncertain'))", params![result.id,device], |r| r.get(0))?;
                    if changed == 1 || terminal {
                        reply.acknowledged.push(result.id);
                    }
                }
                if state == "approved" && active.is_none() {
                    let row: Option<(String,String)> = tx.query_row("SELECT id,body FROM commands WHERE device=?1 AND status IN ('queued','delivered') ORDER BY seq LIMIT 1", [&device], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
                    if let Some((id, body)) = row {
                        tx.execute(
                            "UPDATE commands SET status='delivered',updated=?1 WHERE id=?2",
                            params![now(), id],
                        )?;
                        reply.command = Some(Envelope {
                            id,
                            operation: serde_json::from_str(&body)?,
                        });
                    }
                }
                tx.commit()?;
                Ok(reply)
            }
        }
    }
    pub fn set_device(&mut self, device: &str, approved: bool) -> Result<()> {
        identity::valid_peer(device)?;
        let tx = self.db.transaction()?;
        let state = if approved { "approved" } else { "revoked" };
        ensure!(
            tx.execute(
                "UPDATE devices SET state=?1 WHERE id=?2",
                params![state, device]
            )? == 1,
            "unknown device"
        );
        if !approved {
            tx.execute("UPDATE commands SET status='cancelled',updated=?1 WHERE device=?2 AND status='queued'", params![now(),device])?;
        }
        tx.execute(
            "INSERT INTO audit(action,device,created) VALUES(?1,?2,?3)",
            params![state, device, now()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn certificate(&self, device: &str) -> Result<Vec<u8>> {
        identity::valid_peer(device)?;
        let text: String = self
            .db
            .query_row(
                "SELECT certificate FROM devices WHERE id=?1 AND state='approved'",
                [device],
                |r| r.get(0),
            )
            .optional()?
            .context("device is not approved")?;
        Ok(serde_json::from_str(&text)?)
    }
    pub fn queue(&mut self, commands: Vec<(String, Operation)>) -> Result<Vec<String>> {
        let tx = self.db.transaction()?;
        let mut ids = vec![];
        for (device, operation) in commands {
            let approved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1 AND state='approved')",
                [&device],
                |r| r.get(0),
            )?;
            ensure!(approved, "device is not approved");
            let count: i64 = tx.query_row("SELECT count(*) FROM commands WHERE device=?1 AND status IN ('queued','delivered')", [&device], |r| r.get(0))?;
            ensure!(count < 256, "too many pending commands on this device");
            let id = random_id()?;
            tx.execute("INSERT INTO commands(id,device,body,status,created,updated) VALUES(?1,?2,?3,'queued',?4,?4)", params![id,device,serde_json::to_string(&operation)?,now()])?;
            ids.push(id);
        }
        tx.commit()?;
        Ok(ids)
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        ensure!(
            self.db.execute(
                "UPDATE commands SET status='cancelled',updated=?1 WHERE id=?2 AND status='queued'",
                params![now(), id]
            )? == 1,
            "only an undispatched command can be cancelled"
        );
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let mut statement = self
            .db
            .prepare("SELECT id,name,state,seen,report FROM devices ORDER BY name,id")?;
        let devices = statement.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?)))?
            .map(|row| -> Result<Value> { let (id,name,state,seen,report) = row?; Ok(json!({"id":id,"name":name,"state":state,"last_seen":seen,"online":now().saturating_sub(seen)<20,"report":serde_json::from_str::<Value>(&report)?})) }).collect::<Result<Vec<_>>>()?;
        let mut statement = self.db.prepare("SELECT id,device,body,status,result,created,updated FROM commands ORDER BY seq DESC LIMIT 100")?;
        let commands = statement.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?)))?
            .map(|row| -> Result<Value> { let (id,device,body,status,result,created,updated) = row?; Ok(json!({"id":id,"device":device,"operation":serde_json::from_str::<Value>(&body)?,"status":status,"result":result.map(|s| serde_json::from_str::<Value>(&s)).transpose()?,"created":created,"updated":updated})) }).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"devices":devices,"commands":commands,"server_time":now(),"version":env!("CARGO_PKG_VERSION")}),
        )
    }
    pub fn command(&self, id: &str) -> Result<Value> {
        let (status, result): (String, Option<String>) = self
            .db
            .query_row(
                "SELECT status,result FROM commands WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .context("unknown command")?;
        Ok(
            json!({"id":id,"status":status,"result":result.map(|s| serde_json::from_str::<Value>(&s)).transpose()?}),
        )
    }
}
