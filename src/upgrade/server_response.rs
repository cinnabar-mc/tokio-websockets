//! A [`Codec`] to perform a HTTP Upgrade handshake with a server and validate
//! the response.
use std::{num::NonZeroUsize, str::FromStr};

use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::{Buf, BytesMut};
use http::{HeaderValue, StatusCode, header::HeaderName};
use httparse::{Header, Response};
use tokio_util::codec::Decoder;

use crate::{sha::digest, upgrade::Error};

/// HTTP status code for Switching Protocols.
const SWITCHING_PROTOCOLS: u16 = 101;

/// Find a header in an array of headers by name, ignoring ASCII case.
fn header<'a, 'header: 'a>(
    headers: &'a [Header<'header>],
    name: &'static str,
) -> Result<&'header [u8], Error> {
    let header = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .ok_or(Error::MissingHeader(name))?;

    Ok(header.value)
}

/// [`Decoder`] for parsing the server's response to the client's HTTP
/// `Connection: Upgrade` request.
pub struct Codec {
    /// The SHA-1 digest of the `Sec-WebSocket-Key` header.
    ws_accept: [u8; 20],
    max_response_size: NonZeroUsize,
    capture_rejected_response: bool,
}

impl Codec {
    /// Returns a new [`Codec`].
    ///
    /// The `key` parameter provides the string passed to the server via the
    /// HTTP `Sec-WebSocket-Key` header.
    #[must_use]
    pub fn new(key: &[u8]) -> Self {
        Self {
            ws_accept: digest(key),
            max_response_size: super::DEFAULT_RESPONSE_SIZE,
            capture_rejected_response: false,
        }
    }

    /// Changes the bounded HTTP prefix without limiting a coalesced WebSocket
    /// frame.
    pub(crate) fn max_response_size(mut self, bytes: NonZeroUsize) -> Self {
        self.max_response_size = bytes;
        self
    }

    /// Retains rejected response metadata only for callers that explicitly
    /// requested it.
    pub(crate) fn capture_rejected_response(mut self, capture: bool) -> Self {
        self.capture_rejected_response = capture;
        self
    }
}

impl Decoder for Codec {
    type Error = crate::Error;
    type Item = super::Response;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut headers = [httparse::EMPTY_HEADER; 25];
        let mut response = Response::new(&mut headers);
        let limit = self.max_response_size.get();
        let status = response
            .parse(&src[..src.len().min(limit)])
            .map_err(Error::Parsing)?;

        if !status.is_complete() {
            if src.len() >= limit {
                return Err(Error::ResponseTooLarge { max_size: limit }.into());
            }
            return Ok(None);
        }

        let response_len = status.unwrap();
        let code = response.code.unwrap();

        if code != SWITCHING_PROTOCOLS && !self.capture_rejected_response {
            return Err(crate::Error::Upgrade(Error::DidNotSwitchProtocols(code)));
        }

        if code == SWITCHING_PROTOCOLS {
            let ws_accept_header = header(response.headers, "Sec-WebSocket-Accept")?;
            let mut ws_accept = [0; 20];
            STANDARD
                .decode_slice_unchecked(ws_accept_header, &mut ws_accept)
                .map_err(|_| Error::WrongWebSocketAccept)?;

            if self.ws_accept != ws_accept {
                return Err(crate::Error::Upgrade(Error::WrongWebSocketAccept));
            }
        }

        let mut parsed_response = http::Response::new(());
        *parsed_response.status_mut() =
            StatusCode::from_u16(code).map_err(|_| Error::Parsing(httparse::Error::Status))?;

        match response.version {
            Some(0) => *parsed_response.version_mut() = http::Version::HTTP_10,
            Some(1) => *parsed_response.version_mut() = http::Version::HTTP_11,
            _ => Err(Error::Parsing(httparse::Error::Version))?,
        }

        let header_map = parsed_response.headers_mut();

        header_map.reserve(response.headers.len());

        for header in response.headers {
            let name = HeaderName::from_str(header.name)
                .map_err(|_| Error::Parsing(httparse::Error::HeaderName))?;
            let value = HeaderValue::from_bytes(header.value)
                .map_err(|_| Error::Parsing(httparse::Error::HeaderValue))?;

            header_map.append(name, value);
        }

        src.advance(response_len);

        if code != SWITCHING_PROTOCOLS {
            return Err(Error::RejectedResponse(Box::new(super::RejectedResponse {
                response: parsed_response,
            }))
            .into());
        }

        Ok(Some(parsed_response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unfinished response cannot retain an arbitrarily growing header
    /// value.
    #[test]
    fn incomplete_upgrade_headers_are_bounded() {
        let mut input = BytesMut::from(&b"HTTP/1.1 101 Switching Protocols\r\nX-Padding: "[..]);
        input.resize(128 * 1024, b'a');
        let mut codec = Codec::new(b"original-upgrade-key");
        assert!(
            codec.decode(&mut input).is_err(),
            "oversized incomplete upgrade headers remained pending"
        );
    }

    /// Rejection metadata is opt-in, retains duplicate headers, and never leaks
    /// through Debug.
    #[test]
    fn rejected_upgrade_preserves_retry_metadata_without_implicit_disclosure() {
        let response = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nRetry-After: 9\r\nSet-Cookie: private-value\r\n\r\nbody";
        let error = Codec::new(b"original-upgrade-key")
            .decode(&mut BytesMut::from(response.as_slice()))
            .unwrap_err();
        assert!(matches!(
            error,
            crate::Error::Upgrade(Error::DidNotSwitchProtocols(429))
        ));
        assert!(error.upgrade_response().is_none());

        let mut input = BytesMut::from(response.as_slice());
        let error = Codec::new(b"original-upgrade-key")
            .capture_rejected_response(true)
            .decode(&mut input)
            .unwrap_err();
        let metadata = error.upgrade_response().unwrap();
        assert_eq!(metadata.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: Vec<_> = metadata.headers().get_all("retry-after").iter().collect();
        assert_eq!(retry, ["7", "9"]);
        assert_eq!(metadata.headers()["set-cookie"], "private-value");
        assert!(!format!("{error:?}").contains("private-value"));
        assert_eq!(input.as_ref(), b"body");
    }

    /// A complete header fits at the exact limit, independent of already
    /// buffered frame bytes.
    #[test]
    fn upgrade_limit_counts_only_the_complete_http_header() {
        let key = b"original-upgrade-key";
        let accept = STANDARD.encode(digest(key));
        let header =
            format!("HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
        let limit = NonZeroUsize::new(header.len()).unwrap();
        let mut input = BytesMut::from(header.as_bytes());
        input.resize(header.len() + 128 * 1024, 0xaa);
        let response = Codec::new(key)
            .max_response_size(limit)
            .decode(&mut input)
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(input.len(), 128 * 1024);
        assert!(input.iter().all(|byte| *byte == 0xaa));

        let too_small = NonZeroUsize::new(header.len() - 1).unwrap();
        assert!(matches!(
            Codec::new(key)
                .max_response_size(too_small)
                .decode(&mut BytesMut::from(header.as_bytes())),
            Err(crate::Error::Upgrade(Error::ResponseTooLarge { .. }))
        ));
    }
}
