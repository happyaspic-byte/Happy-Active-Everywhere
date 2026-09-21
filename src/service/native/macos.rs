use super::*;
const TOOL: &str = "/bin/launchctl";
fn domain() -> Result<String> {
    let uid = checked("/usr/bin/id", &["-u"])?;
    ensure!(
        uid.trim().bytes().all(|b| b.is_ascii_digit()) && !uid.trim().is_empty(),
        "invalid user identifier"
    );
    let domain = format!("gui/{}", uid.trim());
    checked(TOOL, &["print", &domain])
        .context("a macOS GUI login session is required for this user service")?;
    Ok(domain)
}
fn definition(config: &Config) -> Result<(PathBuf, String)> {
    let path = home()?
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", config.id));
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>service</string><string>run</string><string>--state</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer><key>Umask</key><integer>63</integer></dict></plist>\n",
        xml(&config.id),
        xml(config.bootstrap.to_str().unwrap()),
        xml(config.state.to_str().unwrap())
    );
    Ok((path, body))
}
fn loaded(domain: &str, id: &str) -> Result<Option<String>> {
    let output = invoke(TOOL, &["print", &format!("{domain}/{id}")])?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?));
    }
    let error = String::from_utf8_lossy(&output.stderr);
    ensure!(
        error.contains("Could not find service"),
        "cannot inspect launchd service: {error}"
    );
    Ok(None)
}
fn own(config: &Config, domain: &str, create: bool) -> Result<PathBuf> {
    let (path, body) = definition(config)?;
    let had_file = exists(&path)?;
    if !had_file {
        ensure!(
            loaded(domain, &config.id)?.is_none(),
            "launchd label already exists without the owned definition"
        );
    }
    ensure_definition(&path, &body, create)?;
    if let Some(runtime) = loaded(domain, &config.id)? {
        let marker = format!("path = {}", path.display());
        ensure!(
            runtime.lines().any(|line| line.trim() == marker),
            "launchd label belongs to another definition"
        );
    }
    Ok(path)
}
pub(crate) fn install(config: &Config) -> Result<()> {
    let domain = domain()?;
    let agents = home()?.join("Library/LaunchAgents");
    directory(agents.parent().unwrap())?;
    private_dir(&agents)?;
    let path = own(config, &domain, true)?;
    checked("/usr/bin/plutil", &["-lint", path.to_str().unwrap()])?;
    Ok(())
}
pub(crate) fn start(config: &Config) -> Result<()> {
    let domain = domain()?;
    let path = own(config, &domain, false)?;
    regular(&path)?;
    let target = format!("{domain}/{}", config.id);
    checked(TOOL, &["enable", &target])?;
    if loaded(&domain, &config.id)?.is_none() {
        checked(TOOL, &["bootstrap", &domain, path.to_str().unwrap()])?;
    } else {
        checked(TOOL, &["kickstart", &target])?;
    }
    Ok(())
}
pub(crate) fn stop(config: &Config) -> Result<()> {
    let domain = domain()?;
    own(config, &domain, false)?;
    let target = format!("{domain}/{}", config.id);
    checked(TOOL, &["disable", &target])?;
    if loaded(&domain, &config.id)?.is_some() {
        checked(TOOL, &["bootout", &target])?;
    }
    Ok(())
}
pub(crate) fn status(config: &Config) -> Result<Value> {
    let domain = domain()?;
    let path = own(config, &domain, false)?;
    let runtime = loaded(&domain, &config.id)?;
    let disabled = checked(TOOL, &["print-disabled", &domain])?;
    let prefix = format!("\"{}\" => ", config.id);
    let entry = disabled
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix));
    let disabled = match entry.map(|s| s.trim_end_matches(',')) {
        Some("true" | "disabled") => true,
        None | Some("false" | "enabled") => false,
        Some(_) => anyhow::bail!("unrecognized launchd enable state"),
    };
    let running = runtime
        .as_ref()
        .is_some_and(|body| body.lines().any(|line| line.trim() == "state = running"));
    Ok(
        json!({"backend":"launchd","registered":path.exists(),"enabled":path.exists() && !disabled,"running":running,"definition":path}),
    )
}
pub(crate) fn uninstall(config: &Config) -> Result<()> {
    stop(config)?;
    let domain = domain()?;
    let path = own(config, &domain, false)?;
    if path.try_exists()? {
        fs::remove_file(&path)?;
        sync_dir(path.parent().unwrap())?;
    }
    // Leave the label enabled for a future reinstall, with no plist/job present.
    checked(TOOL, &["enable", &format!("{domain}/{}", config.id)])?;
    Ok(())
}
