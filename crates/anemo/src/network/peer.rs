use super::wire::{network_message_frame_codec, read_response, write_request};
use super::OutboundRequestLayer;
use crate::{connection::Connection, PeerId, Request, Response, Result};
use bytes::Bytes;
use futures::future::BoxFuture;
use tokio_util::codec::{FramedRead, FramedWrite};
use tower::{Layer, Service, ServiceExt};

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
        let mut send_stream = FramedWrite::new(send_stream, network_message_frame_codec());
        let mut recv_stream = FramedRead::new(recv_stream, network_message_frame_codec());

        //
        // Write Request
        //

        write_request(&mut send_stream, request).await?;
        send_stream.get_mut().finish().await?;

        //
        // Read Response
        //

        let mut response = read_response(&mut recv_stream).await?;

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
