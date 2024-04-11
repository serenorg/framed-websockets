// https://github.com/de-vri-es/hyper-tungstenite-rs/tree/main/tests

use futures_util::SinkExt;
use futures_util::StreamExt;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use std::net::Ipv6Addr;
use tokio::net::TcpStream;

use assert2::assert;
use assert2::let_assert;

#[tokio::test]
async fn hyper() {
    // Bind a TCP listener to an ephemeral port.
    let_assert!(Ok(listener) = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0u16)).await);
    let_assert!(Ok(bind_addr) = listener.local_addr());

    // Spawn the server in a task.
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);

            tokio::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service_fn(upgrade_websocket))
                    .with_upgrades()
                    .await
                {
                    println!("Error serving connection: {:?}", err);
                }
            });
        }
    });

    // Try to create a websocket connection with the server.
    let_assert!(Ok(stream) = TcpStream::connect(bind_addr).await);
    let_assert!(
        Ok(req) = Request::builder()
            .method("GET")
            .uri("ws://localhost/foo")
            .header("Host", "localhost")
            .header(UPGRADE, "websocket")
            .header(CONNECTION, "upgrade")
            .header(
                "Sec-WebSocket-Key",
                fastwebsockets::handshake::generate_key(),
            )
            .header("Sec-WebSocket-Version", "13")
            .body(Empty::<Bytes>::new())
    );
    let_assert!(Ok((mut stream, _response)) = fastwebsockets::handshake::client(&TokioExecutor::default(), req, stream).await);

    let_assert!(Ok(message) = stream.read_frame().await);
    dbg!("recv");
    assert!(message.opcode == fastwebsockets::OpCode::Binary);
    assert!(message.payload == b"Hello!");

    let_assert!(
        Ok(()) = stream
            .write_frame(fastwebsockets::Frame::text(b"Goodbye!".to_vec().into()))
            .await
    );
    dbg!("sent");

    let_assert!(Ok(close_frame) = stream.read_frame().await);
    assert!(close_frame.opcode == fastwebsockets::OpCode::Close);
}

async fn upgrade_websocket(
    mut request: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, framed_websockets::WebSocketError> {
    assert!(framed_websockets::upgrade::is_upgrade_request(&request) == true);

    let (response, stream) = framed_websockets::upgrade::upgrade_downcast::<_, TcpStream>(&mut request)?;
    tokio::spawn(async move {
        let_assert!(Ok(mut stream) = stream.await);
        assert!(let Ok(()) = stream.send(framed_websockets::Frame::binary(b"Hello!".to_vec().into())).await);
        dbg!("sent");
        let_assert!(Some(Ok(reply)) = stream.next().await);
        dbg!("recv");
        assert!(reply.opcode == framed_websockets::OpCode::Text);
        assert!(reply.payload == "Goodbye!");

        assert!(let Ok(()) = stream.send(framed_websockets::Frame::close_raw(vec![].into())).await);
    });

    Ok(response)
}
