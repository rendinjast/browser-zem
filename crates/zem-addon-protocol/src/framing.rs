//! Framing shared by every side of the addon protocol.
//!
//! Control/RPC messages use newline-delimited JSON. Pane *output* uses a
//! length-prefixed binary frame instead: terminal PTY chunks serialized as
//! JSON integer arrays were the dominant cost on the addon↔host hop, so output
//! streams a raw byte payload to keep the hot path cheap.

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use crate::{JsonMessage, OutputEvent};

/// Binary frame kind for a data (output) event.
const OUTPUT_FRAME_DATA: u8 = 0;
/// Binary frame kind for an exited (stream end) event.
const OUTPUT_FRAME_EXIT: u8 = 1;

/// Serialize a message as a newline-terminated JSON line and write it out.
pub async fn write_message<W>(writer: &mut W, msg: &JsonMessage) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a single message, returning `Ok(None)` when the stream closes.
pub async fn read_message<R>(reader: &mut R) -> Result<Option<JsonMessage>>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(serde_json::from_str(&line)?)),
        Err(e) => Err(e.into()),
    }
}

/// Write one output event as a length-prefixed binary frame:
/// `[kind: u8][payload_len: u32 LE][payload]`.
pub async fn write_output_event<W>(writer: &mut W, event: &OutputEvent) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let kind = if event.exited {
        OUTPUT_FRAME_EXIT
    } else {
        OUTPUT_FRAME_DATA
    };
    let mut buf = Vec::with_capacity(5 + event.data.len());
    buf.push(kind);
    buf.extend_from_slice(&(event.data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&event.data);
    writer.write_all(&buf).await?;
    Ok(())
}

/// Read one output event written by [`write_output_event`], returning
/// `Ok(None)` on a clean EOF at a frame boundary.
pub async fn read_output_event<R>(reader: &mut R) -> Result<Option<OutputEvent>>
where
    R: AsyncReadExt + Unpin,
{
    let mut kind = [0u8; 1];
    match reader.read_exact(&mut kind).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let mut len = [0u8; 4];
    reader.read_exact(&mut len).await?;
    let n = u32::from_le_bytes(len) as usize;
    let mut data = vec![0u8; n];
    reader.read_exact(&mut data).await?;
    Ok(Some(OutputEvent {
        data,
        exited: kind[0] == OUTPUT_FRAME_EXIT,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_event_round_trips_and_clean_eof() {
        let (mut writer, mut reader) = tokio::io::duplex(1 << 20);

        let data_event = OutputEvent {
            data: b"\x1b[31mhello world\x1b[0m".to_vec(),
            exited: false,
        };
        let exit_event = OutputEvent {
            data: Vec::new(),
            exited: true,
        };

        write_output_event(&mut writer, &data_event).await.unwrap();
        write_output_event(&mut writer, &exit_event).await.unwrap();
        drop(writer);

        let first = read_output_event(&mut reader).await.unwrap().expect("data event");
        assert_eq!(first.data, data_event.data);
        assert!(!first.exited);

        let second = read_output_event(&mut reader).await.unwrap().expect("exit event");
        assert!(second.exited);

        assert!(read_output_event(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_payload_round_trips() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        write_output_event(&mut writer, &OutputEvent { data: vec![], exited: false })
            .await
            .unwrap();
        drop(writer);
        let event = read_output_event(&mut reader).await.unwrap().expect("event");
        assert!(event.data.is_empty());
    }
}
