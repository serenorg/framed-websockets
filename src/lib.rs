// Copyright 2024 Conrad Ludgate <conrad@neon.tech>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Singificant parts of this code were taken and inspired by
// https://github.com/denoland/fastwebsockets
// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>

//! _framed_websockets_ is a minimal, fast WebSocket server implementation.
//!
//! [https://github.com/neondatabase/framed-websockets](https://github.com/neondatabase/framed-websockets)
//!
//! Passes the _Autobahn|TestSuite_ and fuzzed with LLVM's _libfuzzer_.
//!
//! You can use it as a raw websocket frame parser and deal with spec compliance yourself, or you can use it as a full-fledged websocket server.
//!
//! # Example
//!
//! ```
//! use tokio::net::TcpStream;
//! use framed_websockets::{WebSocketServer, OpCode};
//! use anyhow::Result;
//! use futures_util::stream::TryStreamExt;
//! use futures_util::sink::SinkExt;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<()> {
//!   let mut ws = WebSocketServer::after_handshake(socket);
//!
//!   while let Some(frame) = ws.try_next().await? {
//!     match frame.opcode {
//!       OpCode::Close => break,
//!       OpCode::Text | OpCode::Binary => {
//!         ws.send(frame).await?;
//!       }
//!       _ => {}
//!     }
//!   }
//!   Ok(())
//! }
//! ```
//!
//! ## Fragmentation
//!
//! `framed_websockets` will give the application raw frames with FIN set. Other
//! crates like `tungstenite` which will give you a single message with all the frames
//! concatenated.
//!
//! Usage of the `max_message_size` with further fragment incoming messages to avoid buffering too much in memory.
//!
//! _permessage-deflate is not supported yet._
//!
//! ## HTTP Upgrades
//!
//! This crate supports handling server-side upgrades. This feature is powered by [hyper](https://docs.rs/hyper).
//!
//! ```
//! use framed_websockets::upgrade::upgrade;
//! use http_body_util::Empty;
//! use hyper::{Request, body::{Incoming, Bytes}, Response};
//! use anyhow::Result;
//!
//! async fn server_upgrade(
//!   mut req: Request<Incoming>,
//! ) -> Result<Response<Empty<Bytes>>> {
//!   let (response, fut) = upgrade(&mut req)?;
//!
//!   tokio::spawn(async move {
//!     let ws = fut.await;
//!     // Do something with the websocket
//!   });
//!
//!   Ok(response)
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod error;
mod frame;
mod mask;
/// HTTP upgrades.
pub mod upgrade;

use bytes::Buf;

use bytes::BytesMut;
use futures_core::Stream;
use futures_sink::Sink;
use mask::unmask;
use pin_project::pin_project;
use std::pin::Pin;
use std::task::ready;
use std::task::Context;
use std::task::Poll;
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;
use tokio_util::codec::Framed;

use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

pub use crate::error::WebSocketError;
pub use crate::frame::Frame;
pub use crate::frame::OpCode;

/// WebSocket protocol implementation over an async stream.
#[pin_project]
pub struct WebSocketServer<S> {
    #[pin]
    framed: Framed<S, WsCodec>,
    obligated_send: Option<Frame>,
    recv: Option<Result<Frame, WebSocketError>>,
}

impl<S> WebSocketServer<S> {
    pub fn after_handshake(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        Self {
            framed: Framed::new(
                stream,
                WsCodec {
                    decode_state: FrameDecoderState::Init,
                    max_message_size: 64 * 1024, // 64KiB
                    is_closed: false,
                },
            ),
            obligated_send: None,
            recv: None,
        }
    }

    /// size before closing the frame and starting a new psuedo continuation frame.
    /// must be a multiple of 4
    pub fn with_max_message_size(mut self, max_message_size: usize) -> Self {
        assert!(
            max_message_size % 4 == 0,
            "max message size must be a multiple of 4"
        );
        self.framed.codec_mut().max_message_size = max_message_size;
        self
    }
}

impl<S: AsyncWrite> WebSocketServer<S> {
    fn flush_obligation(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), WebSocketError>> {
        let mut this = self.project();
        if this.framed.codec().is_closed {
            return Poll::Ready(Ok(()));
        }

        if this.obligated_send.is_some() {
            ready!(this.framed.as_mut().poll_ready(cx))?;
            let item = this.obligated_send.take().unwrap();
            this.framed.as_mut().start_send(item)?;
        }

        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + AsyncWrite> Stream for WebSocketServer<S> {
    type Item = Result<Frame, WebSocketError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        ready!(self.as_mut().flush_obligation(cx))?;
        let mut this = self.as_mut().project();
        if let Some(frame) = this.recv.take() {
            return Poll::Ready(Some(frame));
        }

        let is_closed = this.framed.codec().is_closed;

        while let Some(frame) = ready!(this.framed.as_mut().poll_next(cx)) {
            let (res, obligated_send) = process_frame(frame?);

            if let Some(send_frame) = obligated_send {
                if !is_closed {
                    match this.framed.as_mut().poll_ready(cx) {
                        Poll::Pending => {
                            *this.obligated_send = Some(send_frame);
                            *this.recv = res.transpose();
                            return Poll::Pending;
                        }
                        Poll::Ready(res) => {
                            res?;
                            this.framed.as_mut().start_send(send_frame)?;
                        }
                    }
                }
            }

            if let Some(frame) = res? {
                if is_closed && frame.opcode != OpCode::Close {
                    ready!(self.as_mut().flush_obligation(cx))?;
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some(Ok(frame)));
            }
        }
        Poll::Ready(None)
    }
}

impl<S: AsyncWrite> Sink<Frame> for WebSocketServer<S> {
    type Error = WebSocketError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().flush_obligation(cx))?;
        self.project().framed.poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Frame) -> Result<(), Self::Error> {
        debug_assert!(self.obligated_send.is_none());
        self.project().framed.start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().flush_obligation(cx))?;
        self.project().framed.poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // send any queued reply
        ready!(self.as_mut().flush_obligation(cx))?;
        let mut this = self.as_mut().project();

        // send a close message
        if !this.framed.codec().is_closed {
            ready!(this.framed.as_mut().poll_ready(cx))?;
            this.framed.as_mut().start_send(Frame::close(1001, &[]))?;
        }
        debug_assert!(this.framed.codec().is_closed);

        // close the conn
        this.framed.poll_close(cx)
    }
}

pub(crate) fn process_frame(
    mut frame: Frame,
) -> (Result<Option<Frame>, WebSocketError>, Option<Frame>) {
    match frame.opcode {
        OpCode::Close => {
            // technically we should check the payload... but we don't need it...
            let obligated_send = Frame::close_raw(std::mem::take(&mut frame.payload));
            (Ok(Some(frame)), Some(obligated_send))
        }
        OpCode::Ping => (Ok(None), Some(Frame::pong(frame.payload))),
        // we should technically check for utf8 text, but we don't want text anyway
        _ => (Ok(Some(frame)), None),
    }
}

const MAX_HEADER_SIZE: usize = 14;

enum FrameDecoderState {
    Init,
    Header {
        fin: bool,
        op: OpCode,
        length_code: u8,
    },
    Payload {
        fin: bool,
        op: OpCode,
        len: u64,
        mask: [u8; 4],
    },
}

struct WsCodec {
    decode_state: FrameDecoderState,
    /// size before closing the frame and starting a new psuedo continuation frame.
    /// must be a multiple of 4
    max_message_size: usize,

    is_closed: bool,
}

impl Decoder for WsCodec {
    type Item = Frame;

    type Error = WebSocketError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            self.decode_state = match self.decode_state {
                FrameDecoderState::Init => {
                    // Read the first two bytes
                    if src.remaining() < 2 {
                        src.reserve(2);
                        return Ok(None);
                    }
                    let [a, b] = [src[0], src[1]];

                    if a & 0b01110000 != 0 {
                        return Err(WebSocketError::ReservedBitsNotZero);
                    }

                    let op = frame::OpCode::try_from(a & 0b00001111)?;
                    let fin = a & 0b10000000 != 0;
                    let masked = b & 0b10000000 != 0;
                    let length_code = b & 0x7F;

                    // clients must always mask the payload
                    if !masked {
                        return Err(WebSocketError::UnmaskedFrameFromClient);
                    }

                    if frame::is_control(op) && !fin {
                        return Err(WebSocketError::ControlFrameFragmented);
                    }

                    src.advance(2);
                    FrameDecoderState::Header {
                        op,
                        fin,
                        length_code,
                    }
                }
                FrameDecoderState::Header {
                    length_code,
                    op,
                    fin,
                } => {
                    let extra = match length_code {
                        126 => 2,
                        127 => 8,
                        _ => 0,
                    };

                    let needed = extra + 4;
                    if src.remaining() < needed {
                        src.reserve(needed);
                        return Ok(None);
                    }

                    let bytes = src.split_to(needed);

                    let payload_len = match length_code {
                        126 => u64::from(u16::from_be_bytes(bytes[0..2].try_into().unwrap())),
                        127 => u64::from_be_bytes(bytes[0..8].try_into().unwrap()),
                        _ => u64::from(length_code),
                    };

                    let mask = bytes[extra..extra + 4].try_into().unwrap();

                    if op == OpCode::Ping && payload_len > 125 {
                        return Err(WebSocketError::PingFrameTooLarge);
                    }

                    FrameDecoderState::Payload {
                        fin,
                        op,
                        len: payload_len,
                        mask,
                    }
                }
                FrameDecoderState::Payload { fin, op, len, mask } => {
                    // Meh. Try and work out the max_len that satisfies
                    // * max_len <= len
                    // * max_len <= max_message_size
                    let max_len = if usize::BITS < u64::BITS && (usize::MAX as u64) < len {
                        // len is larger than usize::MAX. gotta split
                        self.max_message_size
                    } else if usize::BITS > u64::BITS && (u64::MAX as usize) < self.max_message_size
                    {
                        // max_message_size is larger than u64::MAX. will never split.
                        len as usize
                    } else {
                        usize::min(len as usize, self.max_message_size)
                    };
                    assert!(u64::try_from(max_len).unwrap() <= len);
                    assert!(max_len <= self.max_message_size);

                    if src.remaining() < max_len {
                        // Reserve a bit more to try to get next frame header and avoid a syscall to read it next time
                        src.reserve(max_len + MAX_HEADER_SIZE);
                        return Ok(None);
                    }

                    let mut payload = src.split_to(max_len);
                    unmask(&mut payload, mask);

                    let frame = Frame::new(max_len as u64 == len && fin, op, payload.freeze());

                    if max_len as u64 == len {
                        self.decode_state = FrameDecoderState::Init;
                    } else {
                        self.decode_state = FrameDecoderState::Payload {
                            fin,
                            op: OpCode::Continuation,
                            len: len - max_len as u64,
                            mask,
                        };
                    }
                    break Ok(Some(frame));
                }
            }
        }
    }
}

impl Encoder<Frame> for WsCodec {
    type Error = WebSocketError;

    fn encode(&mut self, mut frame: Frame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if frame.opcode == OpCode::Close {
            self.is_closed = true;
        } else if self.is_closed {
            return Err(WebSocketError::ConnectionClosed);
        }

        frame.write(dst);

        Ok(())
    }
}
