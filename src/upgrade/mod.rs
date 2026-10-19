//! HTTP upgrade request and response generation and validation helpers.

use std::fmt;
#[cfg(feature = "client")]
use std::num::NonZeroUsize;
#[cfg(feature = "server")]
pub(crate) mod client_request;
#[cfg(feature = "client")]
pub(crate) mod server_response;

/// A parsed HTTP/1.1 101 Switching Protocols response.
/// These responses typically do not contain a body, therefore it is omitted.
#[cfg(feature = "client")]
pub type Response = http::Response<()>;

/// Default byte limit for the response status line and headers, excluding
/// WebSocket frames.
#[cfg(feature = "client")]
pub(crate) const DEFAULT_RESPONSE_SIZE: NonZeroUsize =
    NonZeroUsize::new(64 * 1024).expect("nonzero response limit");

/// A rejected upgrade's status and headers, retained only when explicitly
/// requested. Debug output omits response headers, which may contain sensitive
/// values.
#[cfg(feature = "client")]
pub struct RejectedResponse {
    /// Original parsed metadata; never includes the response body.
    pub(crate) response: Response,
}

#[cfg(feature = "client")]
impl RejectedResponse {
    /// Borrows the original response metadata; the HTTP response body is not
    /// read or retained.
    #[must_use]
    pub fn response(&self) -> &Response {
        &self.response
    }
}

#[cfg(feature = "client")]
impl fmt::Debug for RejectedResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RejectedResponse")
            .field("status", &self.response.status())
            .finish_non_exhaustive()
    }
}

/// Errors that occur during the HTTP upgrade handshake between client and
/// server.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Header required in the request or response is not present.
    MissingHeader(&'static str),
    /// `Upgrade` header sent by the client does not match "websocket".
    UpgradeNotWebSocket,
    /// `Connection` header sent by the client does not contain "Upgrade".
    ConnectionNotUpgrade,
    /// `Sec-WebSocket-Version` header sent by the client is not supported by
    /// the server.
    UnsupportedWebSocketVersion,
    /// Failed to parse client request or server response.
    Parsing(httparse::Error),
    /// Server did not return a HTTP Switching Protocols response.
    DidNotSwitchProtocols(u16),
    /// Server did not switch protocols; the caller requested its original HTTP
    /// metadata.
    #[cfg(feature = "client")]
    RejectedResponse(Box<RejectedResponse>),
    /// The response status line and headers exceeded their configured byte
    /// limit.
    #[cfg(feature = "client")]
    ResponseTooLarge { max_size: usize },
    /// Server returned a `Sec-WebSocket-Accept` that is not compatible with the
    /// `Sec-WebSocket-Key` sent by the client.
    WrongWebSocketAccept,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MissingHeader(header) => {
                f.write_str("missing required header: ")?;
                f.write_str(header)
            }
            Error::UpgradeNotWebSocket => f.write_str("upgrade header value was not websocket"),
            Error::ConnectionNotUpgrade => f.write_str("connection header value was not upgrade"),
            Error::UnsupportedWebSocketVersion => f.write_str("unsupported WebSocket version"),
            Error::Parsing(e) => e.fmt(f),
            Error::DidNotSwitchProtocols(status) => {
                f.write_str("expected HTTP 101 Switching Protocols, got status code ")?;
                f.write_fmt(format_args!("{status}"))
            }
            #[cfg(feature = "client")]
            Error::RejectedResponse(response) => {
                write!(
                    f,
                    "expected HTTP 101 Switching Protocols, got status code {}",
                    response.response.status().as_u16()
                )
            }
            #[cfg(feature = "client")]
            Error::ResponseTooLarge { max_size } => {
                write!(f, "HTTP upgrade response headers exceed {max_size} bytes")
            }
            Error::WrongWebSocketAccept => f.write_str("mismatching Sec-WebSocket-Accept header"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::MissingHeader(_)
            | Error::UpgradeNotWebSocket
            | Error::ConnectionNotUpgrade
            | Error::UnsupportedWebSocketVersion
            | Error::DidNotSwitchProtocols(_)
            | Error::WrongWebSocketAccept => None,
            #[cfg(feature = "client")]
            Error::RejectedResponse(_) | Error::ResponseTooLarge { .. } => None,
            Error::Parsing(e) => Some(e),
        }
    }
}

impl From<httparse::Error> for Error {
    fn from(err: httparse::Error) -> Self {
        Self::Parsing(err)
    }
}
