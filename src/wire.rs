use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::timeout,
};

const MAX_FRAME: usize = 80 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct Timing {
    pub io: Duration,
    pub heartbeat: Duration,
    pub operation: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            io: Duration::from_secs(60),
            heartbeat: Duration::from_secs(5),
            operation: Duration::from_secs(24 * 60 * 60),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "kebab-case")]
enum Packet<T> {
    Busy,
    Ready(T),
}

async fn packet<S: AsyncWrite + Unpin, T: Serialize>(
    stream: &mut S,
    value: &Packet<T>,
    timing: Timing,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "frame too large");
    timeout(timing.io, async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await??;
    Ok(())
}

pub(crate) async fn send<S: AsyncWrite + Unpin, T: Serialize>(
    stream: &mut S,
    value: T,
    timing: Timing,
) -> Result<()> {
    packet(stream, &Packet::Ready(value), timing).await
}

pub(crate) async fn read<S: AsyncRead + Unpin, T: DeserializeOwned>(
    stream: &mut S,
    timing: Timing,
) -> Result<T> {
    timeout(timing.operation, async {
        loop {
            let size = timeout(timing.io, stream.read_u32()).await?? as usize;
            ensure!(size <= MAX_FRAME, "frame too large");
            let mut bytes = vec![0; size];
            timeout(timing.io, stream.read_exact(&mut bytes)).await??;
            if let Packet::Ready(value) = serde_json::from_slice::<Packet<T>>(&bytes)? {
                return Ok(value);
            }
        }
    })
    .await?
}

pub(crate) async fn work<S, T, F>(stream: &mut S, timing: Timing, operation: F) -> Result<T>
where
    S: AsyncWrite + Unpin,
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let mut task = tokio::task::spawn_blocking(operation);
    let mut ticks = tokio::time::interval(timing.heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    timeout(timing.operation, async {
        loop {
            tokio::select! {
                result = &mut task => return result?,
                _ = ticks.tick() => packet::<_, ()>(stream, &Packet::Busy, timing).await?,
            }
        }
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> Timing {
        Timing {
            io: Duration::from_millis(250),
            heartbeat: Duration::from_millis(25),
            operation: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn slow_disk_work_does_not_look_like_a_dead_peer() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let send_result = async {
            let value = work(&mut writer, timing(), || {
                std::thread::sleep(Duration::from_millis(750));
                Ok(42u64)
            })
            .await?;
            send(&mut writer, value, timing()).await
        };
        let (sent, received) = tokio::join!(send_result, read::<_, u64>(&mut reader, timing()));
        assert_eq!(received.unwrap(), 42);
        sent.unwrap();
    }

    #[tokio::test]
    async fn a_silent_peer_still_times_out() {
        let (_writer, mut reader) = tokio::io::duplex(4096);
        assert!(read::<_, u64>(&mut reader, timing()).await.is_err());
    }
}
