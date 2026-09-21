use super::*;
const TOOL: &str = "systemctl";
fn unit(config: &Config) -> String {
    format!("{}.service", config.id)
}
fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}
fn definition(config: &Config) -> Result<(PathBuf, String)> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) => PathBuf::from(path),
        None => home()?.join(".config"),
    };
    valid_path(&base)?;
    let path = base.join("systemd/user").join(unit(config));
    let body = format!(
        "[Unit]\nDescription=Happy Active Everywhere {}\n\n[Service]\nType=simple\nExecStart=:{} service run --state {}\nRestart=always\nRestartSec=3\nTimeoutStopSec=20\nKillMode=control-group\nUMask=0077\n\n[Install]\nWantedBy=default.target\n",
        config.id,
        quote(config.bootstrap.to_str().unwrap()),
        quote(config.state.to_str().unwrap())
    );
    Ok((path, body))
}
fn inspect(config: &Config) -> Result<String> {
    let output = invoke(
        TOOL,
        &[
            "--user",
            "show",
            &unit(config),
            "--property=LoadState,ActiveState,UnitFileState,FragmentPath,MainPID,DropInPaths",
        ],
    )?;
    let body = String::from_utf8(output.stdout)?;
    ensure!(
        output.status.success() || body.lines().any(|s| s == "LoadState=not-found"),
        "systemd user manager unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(body)
}
fn own(config: &Config, create: bool) -> Result<PathBuf> {
    let (path, body) = definition(config)?;
    let state = inspect(config)?;
    let fragment = state
        .lines()
        .find_map(|s| s.strip_prefix("FragmentPath="))
        .unwrap_or("");
    ensure!(
        fragment.is_empty() || fragment == path.to_str().unwrap(),
        "systemd unit belongs to another definition"
    );
    ensure_definition(&path, &body, create)?;
    if !fragment.is_empty() {
        ensure!(
            state.lines().any(|s| s == "DropInPaths="),
            "systemd unit has overrides; preserving it"
        );
        verify_command(config)?;
    }
    Ok(path)
}
fn bus_json(args: &[&str]) -> Result<Value> {
    let mut argv = vec!["--user", "--json=short"];
    argv.extend_from_slice(args);
    Ok(serde_json::from_str(&checked("busctl", &argv)?)?)
}
fn verify_command(config: &Config) -> Result<()> {
    let object = bus_json(&[
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "GetUnit",
        "s",
        &unit(config),
    ])?;
    let path = object["data"][0]
        .as_str()
        .context("invalid systemd unit object")?;
    let command = bus_json(&[
        "get-property",
        "org.freedesktop.systemd1",
        path,
        "org.freedesktop.systemd1.Service",
        "ExecStart",
    ])?;
    let commands = command["data"]
        .as_array()
        .context("invalid systemd ExecStart")?;
    let expected = json!([config.bootstrap, "service", "run", "--state", config.state]);
    ensure!(
        commands.len() == 1
            && commands[0][0] == json!(config.bootstrap)
            && commands[0][1] == expected
            && commands[0][2] == false,
        "systemd loaded command belongs to another definition; preserving it"
    );
    Ok(())
}
pub(crate) fn install(config: &Config) -> Result<()> {
    let (path, _) = definition(config)?;
    inspect(config)?;
    let parent = path.parent().unwrap();
    let base = parent.parent().unwrap().parent().unwrap();
    if !base.try_exists()? {
        fs::create_dir(base)?;
    }
    directory(base)?;
    private_dir(&base.join("systemd"))?;
    private_dir(parent)?;
    own(config, true)?;
    checked(TOOL, &["--user", "daemon-reload"])?;
    Ok(())
}
pub(crate) fn start(config: &Config) -> Result<()> {
    regular(&own(config, false)?)?;
    checked(TOOL, &["--user", "enable", "--now", &unit(config)])?;
    Ok(())
}
pub(crate) fn stop(config: &Config) -> Result<()> {
    let path = own(config, false)?;
    if path.try_exists()? {
        checked(TOOL, &["--user", "disable", "--now", &unit(config)])?;
    }
    Ok(())
}
pub(crate) fn status(config: &Config) -> Result<Value> {
    let path = own(config, false)?;
    let state = inspect(config)?;
    let property = |name: &str| {
        state
            .lines()
            .find_map(|s| s.strip_prefix(name))
            .unwrap_or("")
            .to_owned()
    };
    Ok(
        json!({"backend":"systemd","registered":path.exists(),"enabled":property("UnitFileState=")=="enabled","running":property("ActiveState=")=="active","definition":path,"pid":property("MainPID=")}),
    )
}
pub(crate) fn uninstall(config: &Config) -> Result<()> {
    stop(config)?;
    let path = own(config, false)?;
    if path.try_exists()? {
        fs::remove_file(&path)?;
        sync_dir(path.parent().unwrap())?;
    }
    checked(TOOL, &["--user", "daemon-reload"])?;
    Ok(())
}
