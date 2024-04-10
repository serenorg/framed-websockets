use thiserror::Error;

#[derive(Error, Debug)]
pub enum WebSocketError {
    #[error("Unmasked frame from client")]
    UnmaskedFrameFromClient,
    #[error("Connection is closed")]
    ConnectionClosed,
    #[error("Reserved bits are not zero")]
    ReservedBitsNotZero,
    #[error("Control frame must not be fragmented")]
    ControlFrameFragmented,
    #[error("Ping frame too large")]
    PingFrameTooLarge,
    #[error("Frame too large")]
    FrameTooLarge,
    #[error("Sec-Websocket-Version must be 13")]
    InvalidSecWebsocketVersion,
    #[error("Invalid value")]
    InvalidValue,
    #[error("Sec-WebSocket-Key header is missing")]
    MissingSecWebSocketKey,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    HTTPError(#[from] hyper::Error),
}
