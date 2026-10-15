//! A single request/response client for the addon JSON protocol.
//!
//! Used by the zem host to talk to addon processes and by addons to talk to
//! the host's addon DB service. Requests are serialized over one persistent
//! connection that is transparently reconnected on failure.

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex};

use crate::framing::{read_message, read_output_event, write_message};
use crate::{JsonMessage, OutputEvent};

struct Inner {
    socket_path: String,
    reader: Option<AsyncBufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: Option<tokio::net::unix::OwnedWriteHalf>,
    next_id: u64,
}

#[derive(Clone)]
pub struct JsonClient {
    inner: Arc<Mutex<Inner>>,
}

impl JsonClient {
    pub fn new(socket_path: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                socket_path,
                reader: None,
                writer: None,
                next_id: 1,
            })),
        }
    }

    async fn connect(inner: &mut Inner) -> Result<()> {
        let (read_half, write_half) = UnixStream::connect(&inner.socket_path).await?.into_split();
        inner.reader = Some(AsyncBufReader::new(read_half));
        inner.writer = Some(write_half);
        Ok(())
    }

    /// Send a request and await its response.
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut inner = self.inner.lock().await;

        if inner.reader.is_none() {
            Self::connect(&mut inner).await?;
        }

        let id = inner.next_id;
        inner.next_id += 1;
        let msg = JsonMessage::request(id, method, params);
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');

        // Send, reconnecting once if the connection is dead.
        if let Err(_) = {
            let writer = inner.writer.as_mut().expect("connected");
            writer.write_all(line.as_bytes()).await
        } {
            inner.reader = None;
            inner.writer = None;
            Self::connect(&mut inner).await?;
            let writer = inner.writer.as_mut().expect("connected");
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await?;
        } else {
            let writer = inner.writer.as_mut().expect("connected");
            writer.flush().await?;
        }

        // Read the response, resending the request on a dropped connection so
        // the request is never silently lost.
        let resp = loop {
            let reader = inner.reader.as_mut().expect("connected");
            let mut response_line = String::new();
            match reader.read_line(&mut response_line).await {
                Ok(0) => {
                    inner.reader = None;
                    inner.writer = None;
                    Self::connect(&mut inner).await?;
                    let writer = inner.writer.as_mut().expect("connected");
                    writer.write_all(line.as_bytes()).await?;
                    writer.flush().await?;
                }
                Ok(_) => break serde_json::from_str::<JsonMessage>(&response_line)?,
                Err(e) => return Err(e.into()),
            }
        };

        if let Some(err) = resp.error {
            return Err(anyhow::Error::new(err));
        }
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// Open a dedicated streaming connection and forward `output_event`
    /// messages for a pane into the returned channel.
    pub async fn subscribe_output(&self, pane_id: i32) -> Result<mpsc::Receiver<OutputEvent>> {
        let socket_path = self.inner.lock().await.socket_path.clone();
        let stream = UnixStream::connect(&socket_path).await?;
        let (reader, mut writer) = stream.into_split();

        let msg = JsonMessage::request(
            1,
            "subscribe_output",
            serde_json::json!({ "pane_id": pane_id }),
        );
        write_message(&mut writer, &msg).await?;

        let mut buf_reader = AsyncBufReader::new(reader);
        let ack = match read_message(&mut buf_reader).await? {
            Some(m) => m,
            None => {
                return Err(anyhow::anyhow!(
                    "connection closed before subscribe_output ack"
                ))
            }
        };
        if let Some(err) = ack.error {
            return Err(anyhow::Error::new(err));
        }

        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            while let Some(event) = read_output_event(&mut buf_reader).await.ok().flatten() {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}
