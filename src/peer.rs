use crate::network::network_message_frame_codec;
use crate::wire::{read_response, write_request};
use crate::{Connection, PeerId, Request};
use crate::{Response, Result};
use bytes::Bytes;
use futures::future::BoxFuture;
use std::marker::PhantomData;
use tokio_util::codec::{FramedRead, FramedWrite};
use tower::Service;

#[derive(Clone)]
pub struct Peer<'network> {
    connection: Connection,
    _marker: PhantomData<&'network ()>,
}

impl<'network> Peer<'network> {
    pub(crate) fn new(connection: Connection) -> Self {
        Self {
            connection,
            _marker: PhantomData,
        }
    }

    pub fn peer_identity(&self) -> PeerId {
        PeerId(self.connection.peer_identity())
    }

    pub async fn rpc(&self, request: Request<Bytes>) -> Result<Response<Bytes>> {
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
        response.extensions_mut().insert(self.peer_identity());

        Ok(response)
    }

    #[allow(dead_code)]
    async fn message(&self, message: Request<Bytes>) -> Result<()> {
        let send_stream = self.connection.open_uni().await?;
        let mut send_stream = FramedWrite::new(send_stream, network_message_frame_codec());

        //
        // Write Request
        //

        write_request(&mut send_stream, message).await?;
        send_stream.get_mut().finish().await?;

        Ok(())
    }
}

impl<'network> Service<Request<Bytes>> for Peer<'network> {
    type Response = Response<Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'network, Result<Self::Response, Self::Error>>;

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
        Box::pin(async move { peer.rpc(request).await })
    }
}
