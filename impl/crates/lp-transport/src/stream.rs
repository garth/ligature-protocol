//! Framed, typed messages on a QUIC stream. ADR 0036.
//!
//! Wraps a quinn bidirectional stream so callers read and write whole
//! messages. The first message a sender writes is the stream header; the
//! receiver reads it before dispatching.

use lp_core::Error;
use lp_core::frame::{self, MAX_FRAME, StreamHeader};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A bidirectional stream carrying frames.
#[derive(Debug)]
pub struct FramedStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    max: usize,
}

impl FramedStream {
    /// Wraps a stream with the default size limit.
    #[must_use]
    pub fn new(send: quinn::SendStream, recv: quinn::RecvStream) -> Self {
        Self {
            send,
            recv,
            max: MAX_FRAME,
        }
    }

    /// Raises or lowers the per-message limit, for capabilities that define
    /// their own.
    #[must_use]
    pub fn with_max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    /// Writes one message as a frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] for an unencodable or oversize message and
    /// [`Error::Transport`] if the stream is gone.
    pub async fn send<T: Serialize>(&mut self, message: &T) -> Result<(), Error> {
        let bytes = frame::encode(message, self.max)?;
        self.send.write_all(&bytes).await.map_err(|e| match e {
            quinn::WriteError::Stopped(code) => {
                Error::Transport(format!("{STOPPED_BY_PEER} {}", code.into_inner()))
            }
            other => Error::Transport(format!("write: {other}")),
        })
    }

    /// Reads the next frame body. `None` when the peer finished the stream
    /// cleanly between frames.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] for an oversize frame and
    /// [`Error::Transport`] for a truncated frame or a dead stream.
    pub async fn recv_bytes(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let mut prefix = [0u8; 4];
        match self.recv.read_exact(&mut prefix).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
            Err(e) => return Err(Error::Transport(format!("read: {e}"))),
        }
        let len = frame::body_len(prefix, self.max)?;
        let mut body = vec![0u8; len];
        self.recv
            .read_exact(&mut body)
            .await
            .map_err(|e| Error::Transport(format!("read body: {e}")))?;
        Ok(Some(body))
    }

    /// Reads and decodes the next message. `None` when the stream finished.
    ///
    /// # Errors
    ///
    /// As [`Self::recv_bytes`], plus [`Error::Decode`] for a body that is not
    /// a `T`.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Error> {
        match self.recv_bytes().await? {
            Some(body) => frame::decode(&body).map(Some),
            None => Ok(None),
        }
    }

    /// Reads the stream header, which must be the first frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] if the first frame is not a stream header.
    pub async fn recv_header(&mut self) -> Result<StreamHeader, Error> {
        let header: StreamHeader = self
            .recv()
            .await?
            .ok_or_else(|| Error::Decode("stream closed before its header".to_owned()))?;
        if header.message_type.as_str() != frame::STREAM_TYPE {
            return Err(Error::Decode(format!(
                "first frame is {} not a stream header",
                header.message_type
            )));
        }
        Ok(header)
    }

    /// Sets the send side's priority among this connection's streams: quinn
    /// schedules a higher value first and defaults to 0. A bulk data stream
    /// takes a negative value so interactive streams are never behind it.
    pub fn set_priority(&self, priority: i32) {
        let _ = self.send.set_priority(priority);
    }

    /// Finishes the send side; the peer sees a clean end after the last frame.
    pub fn finish(&mut self) {
        let _ = self.send.finish();
    }

    /// Resets the stream with an application error code.
    pub fn reset(&mut self, code: u32) {
        let _ = self.send.reset(code.into());
        let _ = self.recv.stop(code.into());
    }

    /// Stops the receive side only, with an application error code, leaving
    /// what this side has written to be delivered.
    pub fn stop(&mut self, code: u32) {
        let _ = self.recv.stop(code.into());
    }
}

/// How a write error names the peer's stop code; see [`peer_stop_code`].
const STOPPED_BY_PEER: &str = "stopped by peer with code";

/// The application error code the peer stopped the stream with, if `error`
/// came from a write to a stream the peer stopped.
#[must_use]
pub fn peer_stop_code(error: &Error) -> Option<u32> {
    match error {
        Error::Transport(m) => m
            .strip_prefix(STOPPED_BY_PEER)
            .and_then(|rest| rest.trim().parse().ok()),
        _ => None,
    }
}
