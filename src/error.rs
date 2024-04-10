use thiserror::Error;

#[derive(Error, Debug)]
pub enum WebSocketError {
    #[error("Invalid fragment")]
    InvalidFragment,
    #[error("Unmasked frame from client")]
    UnmaskedFrameFromClient,
    #[error("Invalid continuation frame")]
    InvalidContinuationFrame,
    #[error("Invalid status code: {0}")]
    InvalidStatusCode(u16),
    #[error("Invalid upgrade header")]
    InvalidUpgradeHeader,
    #[error("Invalid connection header")]
    InvalidConnectionHeader,
    #[error("Connection is closed")]
    ConnectionClosed,
    #[error("Invalid close frame")]
    InvalidCloseFrame,
    #[error("Invalid close code")]
    InvalidCloseCode,
    #[error("Unexpected EOF")]
    UnexpectedEOF,
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
