//! The frames of the agent link: a big-endian `u32` length, then that many bytes of JSON.

use anyhow::{Context, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The longest frame. A larger payload, such as a build context, follows its frame as raw bytes.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value)?;
    let length = u32::try_from(body.len())
        .ok()
        .filter(|_| body.len() <= MAX_FRAME)
        .context("the frame is too long")?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R, T>(reader: &mut R) -> anyhow::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = usize::try_from(reader.read_u32().await?)?;
    if length > MAX_FRAME {
        bail!("the frame of {length} bytes is too long");
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn a_frame_survives_the_pipe() -> anyhow::Result<()> {
        let (mut writer, mut reader) = duplex(64);
        let value = serde_json::json!({"type": "ping", "text": "ğüşiöç"});
        let sent = value.clone();
        let (written, read) = tokio::join!(write_frame(&mut writer, &sent), async {
            read_frame::<_, serde_json::Value>(&mut reader).await
        });
        written?;
        assert_eq!(read?, value);
        Ok(())
    }

    #[tokio::test]
    async fn a_frame_over_the_limit_is_refused() -> anyhow::Result<()> {
        let (mut writer, mut reader) = duplex(64);
        let length = u32::try_from(MAX_FRAME + 1)?;
        writer.write_all(&length.to_be_bytes()).await?;
        let read = read_frame::<_, serde_json::Value>(&mut reader).await;
        assert!(read.unwrap_err().to_string().contains("too long"));

        let long = "a".repeat(MAX_FRAME);
        assert!(write_frame(&mut writer, &long).await.is_err());
        Ok(())
    }
}
