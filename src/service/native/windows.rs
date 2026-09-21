use super::*;
fn normal(path: &Path) -> Result<String> {
    let value = path.to_str().context("non-Unicode deployment path")?;
    Ok(if let Some(unc) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        value.strip_prefix("\\\\?\\").unwrap_or(value).to_owned()
    })
}
fn quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        result.push_str(&"\\".repeat(if ch == '"' {
            backslashes * 2 + 1
        } else {
            backslashes
        }));
        backslashes = 0;
        result.push(ch);
    }
    result.push_str(&"\\".repeat(backslashes * 2));
    result.push('"');
    result
}
fn file(config: &Config, prefix: &str, extension: &str, bytes: &[u8]) -> Result<PathBuf> {
    let hash = blake3::hash(bytes).to_hex();
    let path = config
        .state
        .join("service")
        .join(format!("{prefix}-{}.{extension}", &hash[..20]));
    if exists(&path)? {
        ensure!(
            read(&path, 128 * 1024)? == bytes,
            "service helper was changed; preserving it"
        );
    } else {
        publish(&path, bytes, false)?;
    }
    Ok(path)
}
fn call(config: &Config, action: &str) -> Result<Value> {
    let arguments = format!("service run --state {}", quote(&normal(&config.state)?));
    let owner = format!(
        "Everywhere {}",
        blake3::hash(&serde_json::to_vec(config)?).to_hex()
    );
    let request = json!({"id":config.id,"executable":normal(&config.bootstrap)?,"arguments":arguments,"owner":owner});
    let request = file(config, "task", "json", &serde_json::to_vec(&request)?)?;
    let script = file(config, "task", "ps1", include_bytes!("../task.ps1"))?;
    let output = checked(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-File",
            script.to_str().unwrap(),
            "-Action",
            action,
            "-Config",
            request.to_str().unwrap(),
        ],
    )?;
    Ok(serde_json::from_str(
        output.trim_start_matches('\u{feff}').trim(),
    )?)
}
pub(crate) fn install(config: &Config) -> Result<()> {
    call(config, "install")?;
    Ok(())
}
pub(crate) fn start(config: &Config) -> Result<()> {
    call(config, "start")?;
    Ok(())
}
pub(crate) fn stop(config: &Config) -> Result<()> {
    call(config, "stop")?;
    Ok(())
}
pub(crate) fn uninstall(config: &Config) -> Result<()> {
    call(config, "uninstall")?;
    Ok(())
}
pub(crate) fn status(config: &Config) -> Result<Value> {
    call(config, "status")
}
