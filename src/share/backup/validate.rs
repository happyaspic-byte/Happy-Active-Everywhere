use super::*;

fn versions(json: &str, nonempty: bool) -> Result<Versions> {
    ensure!(
        json.len() <= 64 * 1024,
        "checkpoint version metadata exceeds 64 KiB"
    );
    let value: Versions = serde_json::from_str(json)?;
    ensure!(
        !nonempty || !value.heads.is_empty(),
        "empty checkpoint version set"
    );
    ensure!(
        Versions::default().join(&value)? == value,
        "invalid checkpoint causal heads"
    );
    Ok(value)
}
pub(super) fn database(connection: &Connection, epoch: &str) -> Result<()> {
    connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=FILE;")?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(
        integrity == "ok",
        "checkpoint SQLite integrity check failed"
    );
    let oversized_schema: i64 = connection.query_row("SELECT count(*) FROM sqlite_schema WHERE length(CAST(name AS BLOB))>128 OR length(CAST(sql AS BLOB))>4096", [], |r| r.get(0))?;
    ensure!(
        oversized_schema == 0,
        "checkpoint schema exceeds metadata limits"
    );
    let tables = [
        ("cursors", "peer,epoch,seq"),
        ("entries", "path,alias,versions,materialized,observed,seq"),
        ("history", "path,id,revision"),
        ("incoming", "path,versions"),
        ("meta", "key,value"),
        ("needed", "hash"),
        ("pending", "path"),
    ];
    let mut schema = connection.prepare("SELECT name,type FROM sqlite_schema WHERE type IN ('table','trigger','view') AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 8")?;
    let actual: Vec<(String, String)> = schema
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        actual
            == tables
                .iter()
                .map(|(name, _)| (name.to_string(), "table".into()))
                .collect::<Vec<_>>(),
        "unexpected checkpoint schema"
    );
    for (table, expected) in tables {
        let mut info = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let names: Vec<String> = info
            .query_map([], |r| r.get(1))?
            .collect::<std::result::Result<_, _>>()?;
        ensure!(
            names.join(",") == expected,
            "unsupported checkpoint table: {table}"
        );
    }
    // Format 1 supports this exact schema. Checking DDL as well as column
    // names retains primary keys, aliases, affinity, nullability and all
    // constraints needed by save() and bounded cursor exchanges.
    for (table, fields) in [
        ("meta", "key TEXT PRIMARY KEY,value TEXT NOT NULL"),
        (
            "entries",
            "path TEXT PRIMARY KEY,alias TEXT UNIQUE NOT NULL,versions TEXT NOT NULL,materialized TEXT NOT NULL,observed TEXT NOT NULL,seq INTEGER NOT NULL",
        ),
        ("pending", "path TEXT PRIMARY KEY"),
        ("incoming", "path TEXT PRIMARY KEY,versions TEXT NOT NULL"),
        ("needed", "hash TEXT PRIMARY KEY"),
        (
            "cursors",
            "peer TEXT PRIMARY KEY,epoch TEXT NOT NULL,seq INTEGER NOT NULL",
        ),
        (
            "history",
            "path TEXT NOT NULL,id TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(path,id)",
        ),
    ] {
        let sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )?;
        let normalize = |s: &str| {
            s.split_whitespace()
                .collect::<String>()
                .to_ascii_lowercase()
        };
        ensure!(
            normalize(&sql) == normalize(&format!("CREATE TABLE {table}({fields})")),
            "unsupported checkpoint constraints: {table}"
        );
    }
    let extra_indexes: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        extra_indexes == 0,
        "unexpected checkpoint index constraints"
    );
    let oversized: i64 = connection.query_row("SELECT
        (SELECT count(*) FROM entries WHERE length(CAST(versions AS BLOB))>65536 OR length(CAST(observed AS BLOB))>65536 OR length(CAST(materialized AS BLOB))>1024 OR length(CAST(path AS BLOB))>16384 OR length(CAST(alias AS BLOB))>32768)
        + (SELECT count(*) FROM history WHERE length(CAST(revision AS BLOB))>65536 OR length(CAST(path AS BLOB))>16384 OR length(id)>64)
        + (SELECT count(*) FROM meta WHERE length(key)>64 OR length(value)>128)", [], |r| r.get(0))?;
    ensure!(oversized == 0, "checkpoint row exceeds metadata limits");
    let stored: String =
        connection.query_row("SELECT value FROM meta WHERE key='epoch'", [], |r| r.get(0))?;
    ensure!(stored == epoch, "checkpoint epoch mismatch");
    let schema: String =
        connection.query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
            r.get(0)
        })?;
    ensure!(schema == "1", "unsupported checkpoint schema version");
    let sequence: String =
        connection.query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0))?;
    let ceiling: u64 = sequence.parse()?;
    ensure!(ceiling <= i64::MAX as u64, "invalid checkpoint sequence");
    let mut entries = connection.prepare(
        "SELECT path,alias,versions,materialized,observed,seq FROM entries ORDER BY path",
    )?;
    for row in entries.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })? {
        let (path, alias, heads, materialized, observed, sequence) = row?;
        validate_path(&path)?;
        ensure!(
            alias == collision_key(&path),
            "checkpoint path alias mismatch"
        );
        versions(&heads, true)?;
        versions(&observed, false)?;
        let materialized: Option<Content> = serde_json::from_str(&materialized)?;
        ensure!(
            materialized != Some(Content::Deleted),
            "invalid checkpoint materialization"
        );
        if let Some(Content::File(hash)) = materialized {
            identity::valid_peer(&hash)?;
        }
        ensure!(
            sequence > 0 && u64::try_from(sequence)? <= ceiling,
            "checkpoint entry sequence is out of range"
        );
    }
    let duplicates: i64 = connection.query_row(
        "SELECT count(*) FROM (SELECT seq FROM entries GROUP BY seq HAVING count(*)>1)",
        [],
        |r| r.get(0),
    )?;
    ensure!(duplicates == 0, "duplicate checkpoint entry sequences");
    let orphans: i64 = connection.query_row(
        "SELECT count(*) FROM pending p LEFT JOIN entries e ON e.path=p.path WHERE e.path IS NULL",
        [],
        |r| r.get(0),
    )?;
    ensure!(orphans == 0, "orphaned checkpoint deletion review");
    let mut history = connection.prepare("SELECT path,id,revision FROM history")?;
    for row in history.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (path, id, json) = row?;
        validate_path(&path)?;
        let revision: Revision = serde_json::from_str(&json)?;
        ensure!(
            revision.id()? == id,
            "checkpoint historical revision hash mismatch"
        );
        Versions::default().join(&Versions {
            heads: vec![revision],
        })?;
    }
    Ok(())
}
