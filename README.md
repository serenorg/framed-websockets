_framed-websockets_ is a fast WebSocket protocol implementation.

Passes the
Autobahn|TestSuite and fuzzed with LLVM's libfuzzer.

You can use it as a raw websocket frame parser and deal with spec compliance
yourself, or you can use it as a full-fledged websocket client/server.

# Example

```rust
use tokio::net::TcpStream;
use framed_websockets::{WebSocketServer, OpCode};
use anyhow::Result;
use futures_util::stream::TryStreamExt;
use futures_util::sink::SinkExt;

async fn handle(
  socket: TcpStream,
) -> Result<()> {
  let mut ws = WebSocketServer::after_handshake(socket);

  while let Some(frame) = ws.try_next().await? {
    match frame.opcode {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        ws.send(frame).await?;
      }
      _ => {}
    }
  }
  Ok(())
}
```

## Fragmentation

`framed_websockets` will give the application raw frames with FIN set. Other
crates like `tungstenite` which will give you a single message with all the frames
concatenated.

Usage of the `max_message_size` with further fragment incoming messages to avoid buffering too much
in memory.

_permessage-deflate is not supported yet._

## HTTP Upgrades

This crate supports handling server-side upgrades. This feature is powered by [hyper](https://docs.rs/hyper).

```rust
use framed_websockets::upgrade::upgrade;
use http_body_util::Empty;
use hyper::{Request, body::{Incoming, Bytes}, Response};
use anyhow::Result;

async fn server_upgrade(
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>> {
  let (response, fut) = upgrade(&mut req)?;

  tokio::spawn(async move {
    let ws = fut.await;
    // Do something with the websocket
  });

  Ok(response)
}
```
