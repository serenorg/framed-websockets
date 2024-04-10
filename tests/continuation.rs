use framed_websockets::OpCode;
use framed_websockets::WebSocketServer;
use futures_util::TryStreamExt;

use assert2::assert;

#[tokio::test]
async fn continuation() {
    let (client_conn, server_conn) = tokio::io::duplex(1024);
    let mut server_ws = WebSocketServer::after_handshake(client_conn).with_max_message_size(256);

    tokio::spawn(async move {
        use fastwebsockets::{Frame, OpCode, Payload, Role, WebSocket};
        let mut client_ws = WebSocket::after_handshake(server_conn, Role::Client);
        client_ws
            .write_frame(Frame::new(
                false,
                OpCode::Binary,
                None,
                Payload::Borrowed(&[0x42; 2000]),
            ))
            .await
            .unwrap();
        client_ws
            .write_frame(Frame::new(
                true,
                OpCode::Continuation,
                None,
                Payload::Borrowed(&[0x42; 1000]),
            ))
            .await
            .unwrap();
        client_ws
            .write_frame(Frame::close_raw(Payload::Borrowed(&[])))
            .await
            .unwrap();
    });

    let mut buf = vec![];
    while let Some(frame) = server_ws.try_next().await.unwrap() {
        assert!(frame.payload.len() <= 256);
        match frame.opcode {
            OpCode::Binary => {
                assert!(
                    buf.is_empty(),
                    "binary opcode should be the first 'frame' only"
                );
                buf.extend_from_slice(&frame.payload);
            }
            OpCode::Continuation => {
                buf.extend_from_slice(&frame.payload);
            }
            OpCode::Close => break,
            _ => unreachable!(),
        }
    }

    assert_eq!(buf.len(), 3000);
}
