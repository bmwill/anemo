// Wire format

use crate::Result;
use anyhow::bail;
use std::collections::HashMap;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

const ANEMO: &[u8; 5] = b"anemo";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum Version {
    V1 = 1,
}

impl Version {
    pub fn new(version: u16) -> Result<Self> {
        match version {
            1 => Ok(Version::V1),
            _ => Err(anyhow::anyhow!("invalid version {}", version)),
        }
    }

    pub fn to_u16(self) -> u16 {
        self as u16
    }
}

impl Default for Version {
    fn default() -> Self {
        Self::V1
    }
}

#[derive(Default)]
pub struct HeaderMap(HashMap<String, String>);

pub(crate) async fn read_version_frame<T: AsyncRead + Unpin>(
    recv_stream: &mut T,
) -> Result<Version> {
    let mut buf: [u8; 8] = [0; 8];
    recv_stream.read_exact(&mut buf).await?;
    if &buf[0..=4] != ANEMO || buf[7] != 0 {
        bail!("Invalid Protocol Header");
    }
    let version_be_bytes = [buf[5], buf[6]];
    let version = u16::from_be_bytes(version_be_bytes);
    Version::new(version)
}

pub(crate) async fn write_version_frame<T: AsyncWrite + Unpin>(
    send_stream: &mut T,
    version: Version,
) -> Result<()> {
    let mut buf: [u8; 8] = [0; 8];
    buf[0..=4].copy_from_slice(ANEMO);
    buf[5..=6].copy_from_slice(&version.to_u16().to_be_bytes());

    send_stream.write_all(&buf).await?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::{read_version_frame, write_version_frame, Version};

    const HEADER: [u8; 8] = [b'a', b'n', b'e', b'm', b'o', 0, 1, 0];

    #[tokio::test]
    async fn read_version_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&HEADER);

        let version = read_version_frame(&mut buf.as_ref()).await.unwrap();
        assert_eq!(Version::V1, version);
    }

    #[tokio::test]
    async fn read_incorrect_version_header() {
        // ANEMO header incorrect
        let header = [b'h', b't', b't', b'p', b'3', 0, 1, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Reserved byte not 0
        let header = [b'a', b'n', b'e', b'm', b'o', 0, 1, 1];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Version is not 1
        let header = [b'a', b'n', b'e', b'm', b'o', 1, 0, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();
    }

    #[tokio::test]
    async fn write_version_header() {
        let mut buf = Vec::new();

        write_version_frame(&mut buf, Version::V1).await.unwrap();
        assert_eq!(HEADER.as_ref(), buf);
    }
}
