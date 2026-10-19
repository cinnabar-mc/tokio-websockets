//! Receive-capacity controls using the actual frame decoder and buffered
//! reader.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use futures_core::Stream;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::codec::FramedRead;

use super::{Limits, Role, WebSocketProtocol};
use crate::Error;

/// Synthetic declaration large enough to distinguish headroom from a full-body
/// reserve.
const PERMITTED_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Supplies one valid binary-frame header, then parks without EOF or payload
/// bytes.
struct HeaderThenPending {
    /// Encoded declaration; no payload bytes are stored by the fixture.
    header: Vec<u8>,
    /// Header bytes already supplied to the actual buffered reader.
    offset: usize,
}

impl HeaderThenPending {
    /// Uses a canonical extended length and a zero mask for the server role.
    fn new(length: usize, masked: bool) -> Self {
        let mut header = vec![0x82, if masked { 0xff } else { 0x7f }];
        header.extend_from_slice(&u64::try_from(length).unwrap().to_be_bytes());
        if masked {
            header.extend_from_slice(&[0; 4]);
        }
        Self { header, offset: 0 }
    }
}

impl AsyncRead for HeaderThenPending {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset == self.header.len() {
            return Poll::Pending;
        }
        let count = output.remaining().min(self.header.len() - self.offset);
        output.put_slice(&self.header[self.offset..self.offset + count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}

/// A permitted declaration does not reserve its body while the actual reader
/// remains pending.
#[test]
fn incomplete_frame_capacity_tracks_received_bytes() {
    for role in [Role::Server, Role::Client] {
        let reader = HeaderThenPending::new(PERMITTED_PAYLOAD_BYTES, role == Role::Server);
        let header_bytes = reader.header.len();
        let codec = WebSocketProtocol::new(
            role,
            Limits::default().max_payload_len(Some(PERMITTED_PAYLOAD_BYTES)),
        );
        let mut framed = FramedRead::new(reader, codec);
        let initial_capacity = framed.read_buffer().capacity();
        let ceiling = initial_capacity.checked_mul(2).unwrap();
        assert!(header_bytes < initial_capacity);
        assert!(ceiling < PERMITTED_PAYLOAD_BYTES / 8);
        let mut cx = Context::from_waker(Waker::noop());

        for _ in 0..3 {
            assert!(Pin::new(&mut framed).poll_next(&mut cx).is_pending());
            assert_eq!(framed.read_buffer().len(), header_bytes);
            assert_eq!(framed.get_ref().offset, header_bytes);
            let capacity = framed.read_buffer().capacity();
            assert!(
                capacity <= ceiling,
                "header-only frame reserved its declared payload: capacity {capacity}, initial {initial_capacity}",
            );
        }
    }
}

/// Over-limit declarations retain their original rejection before reserving
/// body storage.
#[test]
fn oversized_frame_declaration_is_rejected_before_payload_reservation() {
    for role in [Role::Server, Role::Client] {
        let reader = HeaderThenPending::new(PERMITTED_PAYLOAD_BYTES + 1, role == Role::Server);
        let codec = WebSocketProtocol::new(
            role,
            Limits::default().max_payload_len(Some(PERMITTED_PAYLOAD_BYTES)),
        );
        let mut framed = FramedRead::new(reader, codec);
        let initial_capacity = framed.read_buffer().capacity();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut framed).poll_next(&mut cx),
            Poll::Ready(Some(Err(Error::PayloadTooLong { len, max_len })))
                if len == PERMITTED_PAYLOAD_BYTES + 1 && max_len == PERMITTED_PAYLOAD_BYTES
        ));
        assert!(framed.read_buffer().capacity() <= initial_capacity.checked_mul(2).unwrap());
    }
}

/// Full permitted frames survive segmented reads in both roles without losing
/// the next frame.
#[cfg(all(feature = "client", feature = "server"))]
#[tokio::test]
async fn full_size_segmented_messages_preserve_payload_and_boundaries() {
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};

    use crate::{ClientBuilder, Config, Message, ServerBuilder};

    let expected = Bytes::from(
        (0..PERMITTED_PAYLOAD_BYTES)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect::<Vec<_>>(),
    );
    let following = Bytes::from_static(b"next-frame");
    let (client_io, server_io) = tokio::io::duplex(8191);
    let config = Config::default().frame_size(PERMITTED_PAYLOAD_BYTES);
    let limits = Limits::default().max_payload_len(Some(PERMITTED_PAYLOAD_BYTES));
    let mut client = ClientBuilder::new()
        .config(config)
        .limits(limits)
        .take_over(client_io);
    let mut server = ServerBuilder::new()
        .config(config)
        .limits(limits)
        .serve(server_io);

    let client_exchange = async {
        client
            .send(Message::binary(expected.clone()))
            .await
            .unwrap();
        client
            .send(Message::binary(following.clone()))
            .await
            .unwrap();
        let message = client.next().await.unwrap().unwrap();
        assert!(message.is_binary());
        let payload = message.into_payload();
        assert_eq!(payload.len(), PERMITTED_PAYLOAD_BYTES);
        assert_eq!(&*payload, &*expected);
        let next = client.next().await.unwrap().unwrap();
        assert!(next.is_binary());
        assert_eq!(&*next.into_payload(), &*following);
    };
    let server_exchange = async {
        let message = server.next().await.unwrap().unwrap();
        assert!(message.is_binary());
        let payload = message.into_payload();
        assert_eq!(payload.len(), PERMITTED_PAYLOAD_BYTES);
        assert_eq!(&*payload, &*expected);
        server.send(Message::binary(payload)).await.unwrap();
        let next = server.next().await.unwrap().unwrap();
        assert!(next.is_binary());
        let payload = next.into_payload();
        assert_eq!(&*payload, &*following);
        server.send(Message::binary(payload)).await.unwrap();
    };
    tokio::join!(client_exchange, server_exchange);
}
