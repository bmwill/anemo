use super::{
    wire::{network_message_frame_codec, read_response, write_request},
    OutboundRequestLayer,
};
use crate::{connection::Connection, PeerId, Request, Response, Result};
use bytes::Bytes;
use futures::future::BoxFuture;
use quinn::{RecvStream, SendStream};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{Layer, Service, ServiceExt};

// A private structure to hold streams and ensure that if it is dropped
// the send side is reset to free retransmit buffers.
struct StreamPair {
    pub send_stream: FramedWrite<SendStream, LengthDelimitedCodec>,
    pub recv_stream: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl Drop for StreamPair {
    fn drop(&mut self) {
        let _ = self.send_stream.get_mut().reset(0u8.into());
    }
}

/// Handle to a connection with a remote Peer.
#[derive(Clone)]
pub struct Peer {
    connection: Connection,
    outbound_request_layer: OutboundRequestLayer,
}

impl Peer {
    pub(crate) fn new(
        connection: Connection,
        outbound_request_layer: OutboundRequestLayer,
    ) -> Self {
        Self {
            connection,
            outbound_request_layer,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.connection.peer_id()
    }

    pub async fn rpc(&mut self, request: Request<Bytes>) -> Result<Response<Bytes>> {
        self.ready().await?.call(request).await
    }

    async fn do_rpc(&self, request: Request<Bytes>) -> Result<Response<Bytes>> {
        let (send_stream, recv_stream) = self.connection.open_bi().await?;
        let send_stream = FramedWrite::new(send_stream, network_message_frame_codec());
        let recv_stream = FramedRead::new(recv_stream, network_message_frame_codec());

        let mut stream_pair = StreamPair {
            send_stream,
            recv_stream,
        };

        //
        // Write Request
        //

        write_request(&mut stream_pair.send_stream, request).await?;
        stream_pair.send_stream.get_mut().finish().await?;

        //
        // Read Response
        //

        let mut response = read_response(&mut stream_pair.recv_stream).await?;

        // Set the PeerId of this peer
        response.extensions_mut().insert(self.peer_id());

        Ok(response)
    }
}

impl Service<Request<Bytes>> for Peer {
    type Response = Response<Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, request: Request<Bytes>) -> Self::Future {
        let peer = self.clone();
        let inner = tower::service_fn(move |request| {
            let peer = peer.clone();
            async move { peer.do_rpc(request).await }
        })
        .boxed();

        let mut service = self.outbound_request_layer.layer(inner);
        service.call(request)
    }
}
