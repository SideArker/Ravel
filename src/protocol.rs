use crate::geometry::{Display, Layout};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const VERSION: u16 = 1;
pub const MAX_FRAME: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Message {
    Hello {
        version: u16,
        name: String,
        pairing: bool,
    },
    Pake(Vec<u8>),
    Tag(Vec<u8>),
    Approved,
    Ready {
        display: Display,
        layout: Layout,
    },
    Enter([f64; 2]),
    Input(Input),
    Leave,
    Ping,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Input {
    Motion {
        x: f64,
        y: f64,
    },
    Key {
        code: u32,
        down: bool,
    },
    Button {
        code: u32,
        down: bool,
    },
    Scroll {
        axis: u8,
        value: f64,
    },
    Modifiers {
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    },
}

pub fn point_valid(p: [f64; 2]) -> bool {
    p.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
impl Input {
    pub fn validate(self) -> Result<Self> {
        let valid = match self {
            Self::Motion { x, y } => point_valid([x, y]),
            Self::Key { code, .. } => {
                (1..=255).contains(&code) && input_event::scancode::Linux::try_from(code).is_ok()
            }
            Self::Button { code, .. } => (0x110..=0x114).contains(&code),
            Self::Scroll { axis, value } => axis <= 1 && value.is_finite() && value.abs() <= 12000.,
            Self::Modifiers {
                depressed,
                latched,
                locked,
                group,
            } => (depressed | latched | locked) <= 255 && group <= 3,
        };
        ensure!(valid, "Invalid input event");
        Ok(self)
    }
}

pub async fn read<R: AsyncRead + Unpin>(r: &mut R) -> Result<Message> {
    let len = r.read_u32().await? as usize;
    ensure!(len > 0 && len <= MAX_FRAME, "Invalid frame length");
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub async fn write<W: AsyncWrite + Unpin>(w: &mut W, message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    ensure!(bytes.len() <= MAX_FRAME, "Message exceeds frame limit");
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_length_rejected_before_body_read() {
        let mut input = &b"\xff\xff\xff\xff"[..];
        assert!(read(&mut input).await.is_err());
        let mut input = &b"\0\0\0\0"[..];
        assert!(read(&mut input).await.is_err());
    }
    #[tokio::test]
    async fn malformed_and_unknown_fields_rejected() {
        for data in [
            b"{}".as_slice(),
            br#"{"Hello":{"version":1,"name":"x","pairing":false,"extra":true}}"#,
        ] {
            let mut frame = (data.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(data);
            assert!(read(&mut frame.as_slice()).await.is_err());
        }
    }
    #[test]
    fn rejects_invalid_input() {
        assert!(Input::Motion { x: f64::NAN, y: 0. }.validate().is_err());
        assert!(Input::Motion { x: 1.1, y: 0. }.validate().is_err());
        assert!(
            Input::Key {
                code: 0,
                down: true
            }
            .validate()
            .is_err()
        );
        assert!(
            Input::Button {
                code: 999,
                down: true
            }
            .validate()
            .is_err()
        );
        assert!(Input::Scroll { axis: 2, value: 1. }.validate().is_err());
        assert!(
            Input::Key {
                code: 30,
                down: true
            }
            .validate()
            .is_ok()
        );
    }
}
