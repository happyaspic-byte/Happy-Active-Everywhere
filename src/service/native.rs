//! Native commands are bounded, shell-free, and scoped to this deployment.
use super::*;
use std::{io, process::Output, thread, time::Instant};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
pub(super) use linux::{install, start, status, stop, uninstall};
#[cfg(target_os = "macos")]
pub(super) use macos::{install, start, status, stop, uninstall};
#[cfg(windows)]
pub(super) use windows::{install, start, status, stop, uninstall};

fn collect(mut input: impl Read) -> io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let take = count.min((64 * 1024usize).saturating_sub(kept.len()));
        kept.extend_from_slice(&buffer[..take]);
    }
    Ok(kept)
}
fn invoke(program: &str, args: &[&str]) -> Result<Output> {
    let mut child = ManagedChild(
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot run native manager {program}"))?,
    );
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let out = thread::spawn(move || collect(stdout));
    let err = thread::spawn(move || collect(stderr));
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.0.kill()?;
            break child.0.wait()?;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = out
        .join()
        .map_err(|_| anyhow::anyhow!("native stdout reader failed"))??;
    let stderr = err
        .join()
        .map_err(|_| anyhow::anyhow!("native stderr reader failed"))??;
    ensure!(!timed_out, "native service command timed out: {program}");
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
fn checked(program: &str, args: &[&str]) -> Result<String> {
    let output = invoke(program, args)?;
    ensure!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
#[cfg(unix)]
fn home() -> Result<PathBuf> {
    let path =
        PathBuf::from(std::env::var_os("HOME").context("current user's HOME is unavailable")?);
    valid_path(&path)?;
    directory(&path)?;
    Ok(path)
}
#[cfg(unix)]
fn ensure_definition(path: &Path, expected: &str, create: bool) -> Result<bool> {
    if exists(path)? {
        ensure!(
            text(path, 64 * 1024)? == expected,
            "native definition was changed; preserving it: {}",
            path.display()
        );
        Ok(true)
    } else if create {
        let parent = path.parent().context("definition needs parent")?;
        // The platform-specific parent is checked before this call.
        directory(parent)?;
        publish(path, expected.as_bytes(), false)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
#[cfg(target_os = "macos")]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
