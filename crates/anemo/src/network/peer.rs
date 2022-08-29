use super::wire::{network_message_frame_codec, read_response, write_request};
use crate::{
    connection::Connection,
    types::request::{IntoRequest, Message},
    PeerId, Request, Response, Result,
};
use bytes::Bytes;
use futures::future::BoxFuture;
use tokio_util::codec::{FramedRead, FramedWrite};
use tower::Service;

#[derive(Clone)]
pub struct Peer {
    connection: Connection,
}

impl Peer {
    pub(crate) fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn peer_id(&self) -> PeerId {
        self.connection.peer_id()
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
        response.extensions_mut().insert(self.peer_id());

        Ok(response)
    }

    async fn message(&self, message: Message<Bytes>) -> Result<()> {
        let send_stream = self.connection.open_uni().await?;
        let mut send_stream = FramedWrite::new(send_stream, network_message_frame_codec());

        //
        // Write Request
        //

        write_request(&mut send_stream, message.into_request()).await?;
        send_stream.get_mut().finish().await?;

        Ok(())
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
        Box::pin(async move { peer.rpc(request).await })
    }
}

impl Service<Message<Bytes>> for Peer {
    type Response = ();
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
    fn call(&mut self, request: Message<Bytes>) -> Self::Future {
        let peer = self.clone();
        Box::pin(async move { peer.message(request).await })
    }
}
