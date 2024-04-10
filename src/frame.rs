// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
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

use bytes::{BufMut, BytesMut};
use core::ops::Deref;

use crate::WebSocketError;

macro_rules! repr_u8 {
    ($(#[$meta:meta])* $vis:vis enum $name:ident {
      $($(#[$vmeta:meta])* $vname:ident $(= $val:expr)?,)*
    }) => {
      $(#[$meta])*
      $vis enum $name {
        $($(#[$vmeta])* $vname $(= $val)?,)*
      }

      impl core::convert::TryFrom<u8> for $name {
        type Error = WebSocketError;

        fn try_from(v: u8) -> Result<Self, Self::Error> {
          match v {
            $(x if x == $name::$vname as u8 => Ok($name::$vname),)*
            _ => Err(WebSocketError::InvalidValue),
          }
        }
      }
    }
}

pub enum Payload<'a> {
  BorrowedMut(&'a mut [u8]),
  Borrowed(&'a [u8]),
  Owned(Vec<u8>),
  Bytes(BytesMut),
}

impl<'a> core::fmt::Debug for Payload<'a> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    f.debug_struct("Payload").field("len", &self.len()).finish()
  }
}

impl Deref for Payload<'_> {
  type Target = [u8];

  fn deref(&self) -> &Self::Target {
    match self {
      Payload::Borrowed(borrowed) => borrowed,
      Payload::BorrowedMut(borrowed_mut) => borrowed_mut,
      Payload::Owned(owned) => owned.as_ref(),
      Payload::Bytes(b) => b.as_ref(),
    }
  }
}

impl<'a> From<&'a mut [u8]> for Payload<'a> {
  fn from(borrowed: &'a mut [u8]) -> Payload<'a> {
    Payload::BorrowedMut(borrowed)
  }
}

impl<'a> From<&'a [u8]> for Payload<'a> {
  fn from(borrowed: &'a [u8]) -> Payload<'a> {
    Payload::Borrowed(borrowed)
  }
}

impl From<Vec<u8>> for Payload<'_> {
  fn from(owned: Vec<u8>) -> Self {
    Payload::Owned(owned)
  }
}

impl From<Payload<'_>> for Vec<u8> {
  fn from(cow: Payload<'_>) -> Self {
    match cow {
      Payload::Borrowed(borrowed) => borrowed.to_vec(),
      Payload::BorrowedMut(borrowed_mut) => borrowed_mut.to_vec(),
      Payload::Owned(owned) => owned,
      Payload::Bytes(b) => Vec::from(b),
    }
  }
}

impl Payload<'_> {
  #[inline(always)]
  pub fn to_mut(&mut self) -> &mut [u8] {
    match self {
      Payload::Borrowed(borrowed) => {
        *self = Payload::Owned(borrowed.to_owned());
        match self {
          Payload::Owned(owned) => owned,
          _ => unreachable!(),
        }
      }
      Payload::BorrowedMut(borrowed) => borrowed,
      Payload::Owned(ref mut owned) => owned,
      Payload::Bytes(b) => b.as_mut(),
    }
  }
}

impl<'a> PartialEq<&'_ [u8]> for Payload<'a> {
  fn eq(&self, other: &&'_ [u8]) -> bool {
    self.deref() == *other
  }
}

impl<'a, const N: usize> PartialEq<&'_ [u8; N]> for Payload<'a> {
  fn eq(&self, other: &&'_ [u8; N]) -> bool {
    self.deref() == *other
  }
}

/// Represents a WebSocket frame.
pub struct Frame {
  /// Indicates if this is the final frame in a message.
  pub fin: bool,
  /// The opcode of the frame.
  pub opcode: OpCode,
  /// The masking key of the frame, if any.
  mask: Option<[u8; 4]>,
  /// The payload of the frame.
  pub payload: BytesMut,
}

const MAX_HEAD_SIZE: usize = 16;

impl Frame {
  /// Creates a new WebSocket `Frame`.
  pub fn new(
    fin: bool,
    opcode: OpCode,
    mask: Option<[u8; 4]>,
    payload: BytesMut,
  ) -> Self {
    Self {
      fin,
      opcode,
      mask,
      payload,
    }
  }

  /// Create a new WebSocket binary `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Binary, None, payload)`.
  pub fn binary(payload: BytesMut) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Binary,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket close `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Close, None, payload)`.
  ///
  /// This method does not check if `code` is a valid close code and `reason` is valid UTF-8.
  pub fn close(code: u16, reason: &[u8]) -> Self {
    let mut payload = BytesMut::with_capacity(2 + reason.len());
    payload.put_u16(code);
    payload.put(reason);

    Self {
      fin: true,
      opcode: OpCode::Close,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket close `Frame` with a raw payload.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Close, None, payload)`.
  ///
  /// This method does not check if `payload` is valid Close frame payload.
  pub fn close_raw(payload: BytesMut) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Close,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket pong `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Pong, None, payload)`.
  pub fn pong(payload: BytesMut) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Pong,
      mask: None,
      payload,
    }
  }

  /// Checks if the frame payload is valid UTF-8.
  pub fn is_utf8(&self) -> bool {
    #[cfg(feature = "simd")]
    return simdutf8::basic::from_utf8(&self.payload).is_ok();

    #[cfg(not(feature = "simd"))]
    return std::str::from_utf8(&self.payload).is_ok();
  }

  pub fn mask(&mut self) {
    if let Some(mask) = self.mask {
      crate::mask::unmask(&mut self.payload, mask);
    } else {
      let mask: [u8; 4] = rand::random();
      crate::mask::unmask(&mut self.payload, mask);
      self.mask = Some(mask);
    }
  }

  /// Unmasks the frame payload in-place. This method does nothing if the frame is not masked.
  ///
  /// Note: By default, the frame payload is unmasked by `WebSocket::read_frame`.
  pub fn unmask(&mut self) {
    if let Some(mask) = self.mask {
      crate::mask::unmask(&mut self.payload, mask);
    }
  }

  /// Formats the frame header into the head buffer. Returns the size of the length field.
  ///
  /// # Panics
  ///
  /// This method panics if the head buffer is not at least n-bytes long, where n is the size of the length field (0, 2, 4, or 10)
  pub fn fmt_head(&mut self, buf: &mut BytesMut) {
    buf.put_u8((self.fin as u8) << 7 | (self.opcode as u8));

    let mask = (self.mask.is_some() as u8) << 7;
    let len = self.payload.len();
    if len < 126 {
      buf.put_u8(len as u8 | mask);
    } else if len < 65536 {
      buf.put_u8(126 | mask);
      buf.put_u16(len as u16);
    } else {
      buf.put_u8(127 | mask);
      buf.put_u64(len as u64);
    };

    if let Some(mask) = self.mask {
      buf.put(&mask[..]);
    }
  }

  /// Writes the frame to the buffer and returns a slice of the buffer containing the frame.
  pub fn write(&mut self, buf: &mut BytesMut) {
    let len = self.payload.len();
    if len > buf.remaining_mut() {
      buf.reserve(len + MAX_HEAD_SIZE - buf.remaining_mut());
    }

    self.fmt_head(buf);
    buf.put(&*self.payload);
  }
}

repr_u8! {
    #[repr(u8)]
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub enum OpCode {
        Continuation = 0x0,
        Text = 0x1,
        Binary = 0x2,
        Close = 0x8,
        Ping = 0x9,
        Pong = 0xA,
    }
}

#[inline]
pub fn is_control(opcode: OpCode) -> bool {
  matches!(opcode, OpCode::Close | OpCode::Ping | OpCode::Pong)
}
