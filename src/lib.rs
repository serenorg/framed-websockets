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
//! [https://github.com/conradludgate/fastwebsockets](https://github.com/conradludgate/fastwebsockets)
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

mod close;
mod error;
mod frame;
mod mask;
/// HTTP upgrades.
pub mod upgrade;

use bytes::Buf;

use bytes::BytesMut;
use futures_core::Stream;
use futures_sink::Sink;
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

pub use crate::close::CloseCode;
pub use crate::error::WebSocketError;
pub use crate::frame::Frame;
pub use crate::frame::OpCode;
pub use crate::mask::unmask;

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
                    max_message_size: 64 << 20,
                    is_closed: false,
                },
            ),
            obligated_send: None,
            recv: None,
        }
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

        if let Some(frame) = this.obligated_send.take() {
            match this.framed.as_mut().poll_ready(cx) {
                Poll::Pending => {
                    *this.obligated_send = Some(frame);
                    return Poll::Pending;
                }
                Poll::Ready(res) => {
                    res?;
                    this.framed.as_mut().start_send(frame)?;
                }
            }
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
            this.framed
                .as_mut()
                .start_send(Frame::close(CloseCode::Away.into(), &[]))?;
        }

        // close the conn
        this.framed.poll_close(cx)
    }
}

pub(crate) fn process_frame(
    mut frame: Frame,
) -> (Result<Option<Frame>, WebSocketError>, Option<Frame>) {
    match frame.opcode {
        OpCode::Close => {
            match frame.payload.len() {
                0 => {}
                1 => return (Err(WebSocketError::InvalidCloseFrame), None),
                _ => {
                    let code = close::CloseCode::from(u16::from_be_bytes(
                        frame.payload[0..2].try_into().unwrap(),
                    ));

                    if !code.is_allowed() {
                        return (
                            Err(WebSocketError::InvalidCloseCode),
                            Some(Frame::close_raw(frame.payload)),
                        );
                    }
                }
            };

            let obligated_send = Frame::close_raw(std::mem::take(&mut frame.payload));
            (Ok(Some(frame)), Some(obligated_send))
        }
        OpCode::Ping => (Ok(None), Some(Frame::pong(frame.payload))),
        OpCode::Text => {
            if frame.fin && !frame.is_utf8() {
                (Err(WebSocketError::InvalidUTF8), None)
            } else {
                (Ok(Some(frame)), None)
            }
        }
        _ => (Ok(Some(frame)), None),
    }
}

const MAX_HEADER_SIZE: usize = 14;

enum FrameDecoderState {
    Init,
    Header([u8; 2]),
    Payload {
        fin: bool,
        op: OpCode,
        len: usize,
        mask: Option<[u8; 4]>,
    },
}

struct WsCodec {
    decode_state: FrameDecoderState,
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

                    src.advance(2);
                    FrameDecoderState::Header([a, b])
                }
                FrameDecoderState::Header([a, b]) => {
                    let masked = b & 0b10000000 != 0;

                    let length_code = b & 0x7F;
                    let extra = match length_code {
                        126 => 2,
                        127 => 8,
                        _ => 0,
                    };

                    let needed = extra + masked as usize * 4;
                    if src.remaining() < needed {
                        src.reserve(needed);
                        return Ok(None);
                    }

                    let bytes = src.split_to(needed);

                    let masked = b & 0b10000000 != 0;
                    let length_code = b & 0x7F;

                    let payload_len = match length_code {
                        126 => u64::from(u16::from_be_bytes(bytes[0..2].try_into().unwrap())),
                        127 => u64::from_be_bytes(bytes[0..8].try_into().unwrap()),
                        _ => u64::from(length_code),
                    };

                    let payload_len = match usize::try_from(payload_len) {
                        Ok(length) => length,
                        Err(_) => return Err(WebSocketError::FrameTooLarge),
                    };

                    let mask = if masked {
                        Some(bytes[extra..extra + 4].try_into().unwrap())
                    } else {
                        None
                    };

                    let opcode = frame::OpCode::try_from(a & 0b00001111)?;
                    let fin = a & 0b10000000 != 0;

                    if frame::is_control(opcode) && !fin {
                        return Err(WebSocketError::ControlFrameFragmented);
                    }

                    if opcode == OpCode::Ping && payload_len > 125 {
                        return Err(WebSocketError::PingFrameTooLarge);
                    }

                    if payload_len >= self.max_message_size {
                        return Err(WebSocketError::FrameTooLarge);
                    }

                    FrameDecoderState::Payload {
                        fin,
                        op: opcode,
                        len: payload_len,
                        mask,
                    }
                }
                FrameDecoderState::Payload { fin, op, len, mask } => {
                    if src.remaining() < len {
                        // Reserve a bit more to try to get next frame header and avoid a syscall to read it next time
                        src.reserve(len + MAX_HEADER_SIZE);
                        return Ok(None);
                    }

                    let mut payload = src.split_to(len);
                    if let Some(mask) = mask {
                        unmask(&mut payload, mask);
                    }
                    let frame = Frame::new(fin, op, payload.freeze());

                    self.decode_state = FrameDecoderState::Init;
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

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = {
        const fn assert_unsync<S>() {
            // Generic trait with a blanket impl over `()` for all types.
            trait AmbiguousIfImpl<A> {
                // Required for actually being able to reference the trait.
                fn some_item() {}
            }

            impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

            // Used for the specialized impl when *all* traits in
            // `$($t)+` are implemented.
            #[allow(dead_code)]
            struct Invalid;

            impl<T: ?Sized + Sync> AmbiguousIfImpl<Invalid> for T {}

            // If there is only one specialized trait impl, type inference with
            // `_` can be resolved and this can compile. Fails to compile if
            // `$x` implements `AmbiguousIfImpl<Invalid>`.
            let _ = <S as AmbiguousIfImpl<_>>::some_item;
        }
        assert_unsync::<WebSocketServer<tokio::net::TcpStream>>();
    };
}
