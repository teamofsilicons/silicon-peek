//! Framing for the peekd socket (BLUEPRINT §1.6): one UTF-8 JSON object per
//! line, optionally followed by length-declared binary blobs.
//!
//! ```text
//! {"v":1,"id":"…","op":"register.drawing",…,"bin":[3174]}\n<3174 raw bytes>
//! ```
//!
//! If the object has `"bin":[n1,n2,…]`, exactly `n1+n2+…` raw bytes follow the
//! newline; they are the frame's blobs, in order. `bin` is reserved: the codec
//! strips it from [`Frame::header`] and writes it from [`Frame::blobs`].
//!
//! Limits: the JSON line ≤ 1 MiB (duplicate keys refused), each blob ≤ 10 MiB,
//! ≤ 40 MiB of blobs per frame. Per-kind limits (a drawing ≤ 256 KiB, a WAV
//! ≤ 4 MiB, a TTS chunk ≤ 64 KiB) are checked per op; see [`blob_limit`].
//!
//! [`Decoder`] is a pure state machine; [`FrameReader`]/[`write_frame`] wrap it
//! for `std::io`, [`AsyncFrameReader`]/[`write_frame_async`] for tokio.

use std::io::{self, Read, Write};

use bytes::{Buf as _, BytesMut};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::{
    error::{Error, ErrorCode, Origin},
    json::parse_value,
    schema::limits,
};

/// The reserved blob-length field.
pub const BIN_FIELD: &str = "bin";

/// Framing limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLimits {
    /// Bytes in the JSON line, excluding the newline.
    pub max_json: usize,
    /// Bytes in one blob.
    pub max_blob: usize,
    /// Bytes of blobs in one frame.
    pub max_blobs_total: usize,
    /// Blobs in one frame.
    pub max_blob_count: usize,
}

impl FrameLimits {
    /// The protocol v1 limits.
    pub const V1: FrameLimits = FrameLimits {
        max_json: 1024 * 1024,
        max_blob: limits::IMAGE_MAX_BYTES,
        max_blobs_total: 40 * 1024 * 1024,
        max_blob_count: 64,
    };
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self::V1
    }
}

/// The per-kind limit for each blob of a request op or event, if it carries
/// blobs. Replies that carry a PNG preview use [`limits::IMAGE_MAX_BYTES`].
#[must_use]
pub fn blob_limit(name: &str) -> Option<usize> {
    match name {
        "register.drawing" => Some(limits::DRAWING_MAX_BYTES),
        "voice.submit" => Some(limits::WAV_MAX_BYTES),
        "tts.chunk" => Some(limits::TTS_CHUNK_MAX_BYTES),
        "send" => Some(limits::IMAGE_MAX_BYTES),
        _ => None,
    }
}

/// A framing failure.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// Reading or writing the socket failed.
    #[error("IPC I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The peer closed the connection inside a frame.
    #[error("the peer closed the connection in the middle of a frame")]
    UnexpectedEof,
    /// A limit was exceeded.
    #[error("{what} is {actual} bytes; the limit is {limit}")]
    TooLarge {
        /// What was too large.
        what: &'static str,
        /// The limit.
        limit: usize,
        /// The observed size (a lower bound while still streaming).
        actual: usize,
    },
    /// The bytes are not a valid frame.
    #[error("invalid IPC frame: {0}")]
    Malformed(String),
}

impl From<FrameError> for Error {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::Io(io) => Error::new(
                ErrorCode::DaemonUnavailable,
                format!("the peekd connection failed: {io}"),
            )
            .with_hint("check peekd with `peek daemon status`; restart it with `peek daemon restart`")
            .with_origin(Origin::Transport)
            .with_source(io),
            FrameError::UnexpectedEof => Error::new(
                ErrorCode::DaemonUnavailable,
                "peekd closed the connection in the middle of a frame",
            )
            .with_hint("peekd may have restarted; retry the command")
            .with_origin(Origin::Transport),
            FrameError::TooLarge {
                what,
                limit,
                actual,
            } => Error::new(
                ErrorCode::FrameTooLarge,
                format!("{what} is {actual} bytes; the IPC limit is {limit}"),
            ),
            FrameError::Malformed(m) => Error::new(
                ErrorCode::ProtocolError,
                format!("invalid IPC frame: {m}"),
            )
            .with_hint("the CLI and peekd disagree about the protocol; update both with `apps update 'peek'`"),
        }
    }
}

/// One frame: the JSON header (without `bin`) and its blobs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// The JSON object, minus the reserved `bin` field.
    pub header: Map<String, Value>,
    /// The binary blobs, in order.
    pub blobs: Vec<Vec<u8>>,
}

impl Frame {
    /// A frame without blobs.
    #[must_use]
    pub fn new(header: Map<String, Value>) -> Self {
        Self {
            header,
            blobs: Vec::new(),
        }
    }

    /// Total blob bytes.
    #[must_use]
    pub fn blob_bytes(&self) -> usize {
        self.blobs.iter().map(Vec::len).sum()
    }
}

/// Serializes a frame's header line (including the trailing newline). The
/// blobs are written after it, unchanged.
///
/// # Errors
/// [`FrameError::Malformed`] when the header already has `bin`;
/// [`FrameError::TooLarge`] when a limit is exceeded.
pub fn encode_header(frame: &Frame, limits: &FrameLimits) -> Result<Vec<u8>, FrameError> {
    if frame.header.contains_key(BIN_FIELD) {
        return Err(FrameError::Malformed(
            "the header must not set `bin`; pass blobs separately".into(),
        ));
    }
    check_blobs(frame.blobs.iter().map(Vec::len), limits)?;
    let mut line = if frame.blobs.is_empty() {
        serde_json::to_vec(&frame.header)
    } else {
        let mut header = frame.header.clone();
        header.insert(
            BIN_FIELD.to_owned(),
            Value::Array(frame.blobs.iter().map(|b| Value::from(b.len())).collect()),
        );
        serde_json::to_vec(&header)
    }
    .map_err(|e| FrameError::Malformed(format!("the header cannot be serialized: {e}")))?;
    if line.len() > limits.max_json {
        return Err(FrameError::TooLarge {
            what: "the frame's JSON line",
            limit: limits.max_json,
            actual: line.len(),
        });
    }
    line.push(b'\n');
    Ok(line)
}

/// Serializes a whole frame into one buffer.
///
/// # Errors
/// As [`encode_header`].
pub fn encode(frame: &Frame, limits: &FrameLimits) -> Result<Vec<u8>, FrameError> {
    let mut out = encode_header(frame, limits)?;
    out.reserve(frame.blob_bytes());
    for b in &frame.blobs {
        out.extend_from_slice(b);
    }
    Ok(out)
}

fn check_blobs(lens: impl Iterator<Item = usize>, limits: &FrameLimits) -> Result<(), FrameError> {
    let mut total = 0usize;
    for (count, len) in lens.enumerate() {
        if count + 1 > limits.max_blob_count {
            return Err(FrameError::TooLarge {
                what: "the frame's blob count",
                limit: limits.max_blob_count,
                actual: count + 1,
            });
        }
        if len > limits.max_blob {
            return Err(FrameError::TooLarge {
                what: "a blob",
                limit: limits.max_blob,
                actual: len,
            });
        }
        total = total.saturating_add(len);
        if total > limits.max_blobs_total {
            return Err(FrameError::TooLarge {
                what: "the frame's blobs",
                limit: limits.max_blobs_total,
                actual: total,
            });
        }
    }
    Ok(())
}

#[derive(Debug)]
enum State {
    Header,
    Blobs {
        header: Map<String, Value>,
        lens: Vec<usize>,
        blobs: Vec<Vec<u8>>,
    },
}

/// Incremental frame decoder: feed it bytes in any split, take frames out.
#[derive(Debug)]
pub struct Decoder {
    limits: FrameLimits,
    buf: BytesMut,
    scanned: usize,
    state: State,
}

impl Decoder {
    /// A decoder with the given limits.
    #[must_use]
    pub fn new(limits: FrameLimits) -> Self {
        Self {
            limits,
            buf: BytesMut::with_capacity(8 * 1024),
            scanned: 0,
            state: State::Header,
        }
    }

    /// Appends received bytes.
    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Whether a partial frame is buffered (EOF now would truncate it).
    #[must_use]
    pub fn is_mid_frame(&self) -> bool {
        !self.buf.is_empty() || matches!(self.state, State::Blobs { .. })
    }

    /// Decodes the next complete frame, if the buffer holds one.
    ///
    /// # Errors
    /// [`FrameError::TooLarge`] or [`FrameError::Malformed`]; the connection
    /// should then be closed, because the stream position is lost.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        loop {
            match &mut self.state {
                State::Header => {
                    let Some(pos) = self.buf[self.scanned..].iter().position(|&b| b == b'\n')
                    else {
                        self.scanned = self.buf.len();
                        if self.buf.len() > self.limits.max_json {
                            return Err(FrameError::TooLarge {
                                what: "the frame's JSON line",
                                limit: self.limits.max_json,
                                actual: self.buf.len(),
                            });
                        }
                        return Ok(None);
                    };
                    let end = self.scanned + pos;
                    self.scanned = 0;
                    if end > self.limits.max_json {
                        return Err(FrameError::TooLarge {
                            what: "the frame's JSON line",
                            limit: self.limits.max_json,
                            actual: end,
                        });
                    }
                    let line = self.buf.split_to(end);
                    self.buf.advance(1);
                    let (header, lens) = parse_header(&line, &self.limits)?;
                    if lens.is_empty() {
                        return Ok(Some(Frame::new(header)));
                    }
                    self.state = State::Blobs {
                        header,
                        blobs: Vec::with_capacity(lens.len()),
                        lens,
                    };
                }
                State::Blobs {
                    header,
                    lens,
                    blobs,
                } => {
                    while blobs.len() < lens.len() {
                        let need = lens[blobs.len()];
                        if self.buf.len() < need {
                            self.buf.reserve(need - self.buf.len());
                            return Ok(None);
                        }
                        blobs.push(self.buf.split_to(need).to_vec());
                    }
                    let frame = Frame {
                        header: std::mem::take(header),
                        blobs: std::mem::take(blobs),
                    };
                    self.state = State::Header;
                    return Ok(Some(frame));
                }
            }
        }
    }
}

fn parse_header(
    line: &[u8],
    limits: &FrameLimits,
) -> Result<(Map<String, Value>, Vec<usize>), FrameError> {
    let value = parse_value(line).map_err(|e| FrameError::Malformed(e.message().to_owned()))?;
    let Value::Object(mut header) = value else {
        return Err(FrameError::Malformed(
            "each line must be one JSON object".into(),
        ));
    };
    let lens = match header.remove(BIN_FIELD) {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| {
                        FrameError::Malformed(
                            "`bin` must be an array of non-negative byte counts".into(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(FrameError::Malformed(
                "`bin` must be an array of byte counts".into(),
            ));
        }
    };
    check_blobs(lens.iter().copied(), limits)?;
    Ok((header, lens))
}

/// Reads frames from a blocking reader.
#[derive(Debug)]
pub struct FrameReader<R> {
    inner: R,
    decoder: Decoder,
    chunk: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    /// Wraps a reader with the v1 limits.
    pub fn new(inner: R) -> Self {
        Self::with_limits(inner, FrameLimits::V1)
    }

    /// Wraps a reader with explicit limits.
    pub fn with_limits(inner: R, limits: FrameLimits) -> Self {
        Self {
            inner,
            decoder: Decoder::new(limits),
            chunk: vec![0; 64 * 1024],
        }
    }

    /// The next frame; `None` on a clean end of stream.
    ///
    /// # Errors
    /// [`FrameError`].
    pub fn read_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        loop {
            if let Some(f) = self.decoder.next_frame()? {
                return Ok(Some(f));
            }
            let n = match self.inner.read(&mut self.chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            if n == 0 {
                return if self.decoder.is_mid_frame() {
                    Err(FrameError::UnexpectedEof)
                } else {
                    Ok(None)
                };
            }
            self.decoder.feed(&self.chunk[..n]);
        }
    }

    /// The wrapped reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

/// Writes one frame to a blocking writer and flushes.
///
/// # Errors
/// [`FrameError`].
pub fn write_frame<W: Write>(
    w: &mut W,
    frame: &Frame,
    limits: &FrameLimits,
) -> Result<(), FrameError> {
    let header = encode_header(frame, limits)?;
    w.write_all(&header)?;
    for b in &frame.blobs {
        w.write_all(b)?;
    }
    w.flush()?;
    Ok(())
}

/// Reads frames from a tokio reader.
#[derive(Debug)]
pub struct AsyncFrameReader<R> {
    inner: R,
    decoder: Decoder,
    chunk: Vec<u8>,
}

impl<R: AsyncRead + Unpin> AsyncFrameReader<R> {
    /// Wraps a reader with the v1 limits.
    pub fn new(inner: R) -> Self {
        Self::with_limits(inner, FrameLimits::V1)
    }

    /// Wraps a reader with explicit limits.
    pub fn with_limits(inner: R, limits: FrameLimits) -> Self {
        Self {
            inner,
            decoder: Decoder::new(limits),
            chunk: vec![0; 64 * 1024],
        }
    }

    /// The next frame; `None` on a clean end of stream. Cancel-safe between
    /// frames only when no partial frame is buffered.
    ///
    /// # Errors
    /// [`FrameError`].
    pub async fn read_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        loop {
            if let Some(f) = self.decoder.next_frame()? {
                return Ok(Some(f));
            }
            let n = match self.inner.read(&mut self.chunk).await {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            if n == 0 {
                return if self.decoder.is_mid_frame() {
                    Err(FrameError::UnexpectedEof)
                } else {
                    Ok(None)
                };
            }
            self.decoder.feed(&self.chunk[..n]);
        }
    }

    /// The wrapped reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

/// Writes one frame to a tokio writer and flushes.
///
/// # Errors
/// [`FrameError`].
pub async fn write_frame_async<W: AsyncWrite + Unpin>(
    w: &mut W,
    frame: &Frame,
    limits: &FrameLimits,
) -> Result<(), FrameError> {
    let header = encode_header(frame, limits)?;
    w.write_all(&header).await?;
    for b in &frame.blobs {
        w.write_all(b).await?;
    }
    w.flush().await?;
    Ok(())
}
