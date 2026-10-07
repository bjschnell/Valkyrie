//! Length-prefixed JSON framing.

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME {
        bail!("frame too large: {} bytes", body.len());
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    w.write_all(&encode(msg)?).await?;
    Ok(())
}

/// Returns `Ok(None)` on clean EOF at a frame boundary.
pub async fn read_frame<R, T>(r: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("frame too large: {len} bytes");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.context("truncated frame")?;
    Ok(Some(
        serde_json::from_slice(&body).context("malformed frame")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[tokio::test]
    async fn round_trip() {
        let msgs = vec![
            ClientMsg::Spawn {
                req: 1,
                spec: SpawnSpec {
                    command: vec!["claude".into()],
                    cwd: Some("/tmp".into()),
                    name: None,
                    size: Size { cols: 80, rows: 24 },
                },
            },
            ClientMsg::Input {
                session: 3,
                data: vec![0x1b, b'[', b'A', 0xff],
            },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m).await.unwrap();
        }
        let mut rd = buf.as_slice();
        for m in &msgs {
            let got: ClientMsg = read_frame(&mut rd).await.unwrap().unwrap();
            assert_eq!(&got, m);
        }
        assert!(read_frame::<_, ClientMsg>(&mut rd).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_oversized_length() {
        let frame = (MAX_FRAME as u32 + 1).to_be_bytes();
        let mut rd = &frame[..];
        assert!(read_frame::<_, ClientMsg>(&mut rd).await.is_err());
    }

    #[tokio::test]
    async fn truncated_body_is_error() {
        let mut frame = encode(&ClientMsg::List { req: 7 }).unwrap();
        frame.truncate(frame.len() - 1);
        let mut rd = frame.as_slice();
        assert!(read_frame::<_, ClientMsg>(&mut rd).await.is_err());
    }
}
