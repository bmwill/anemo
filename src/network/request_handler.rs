use super::{
    wire::{network_message_frame_codec, read_request, write_response},
    ActivePeers,
};
use crate::{connection::Connection, endpoint::NewConnection, PeerId, Request, Response, Result};
use bytes::Bytes;
use futures::{
    stream::{Fuse, FuturesUnordered},
    StreamExt,
};
use quinn::{Datagrams, IncomingBiStreams, IncomingUniStreams, RecvStream, SendStream};
use std::convert::Infallible;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::{info, trace, warn};

pub struct InboundRequestHandler {
    connection: Connection,
    incoming_bi: Fuse<IncomingBiStreams>,
    incoming_uni: Fuse<IncomingUniStreams>,
    incoming_datagrams: Fuse<Datagrams>,

    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    active_peers: ActivePeers,
}

impl InboundRequestHandler {
    pub fn new(
        NewConnection {
            connection,
            uni_streams,
            bi_streams,
            datagrams,
        }: NewConnection,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        active_peers: ActivePeers,
    ) -> Self {
        Self {
            connection,
            incoming_uni: uni_streams.fuse(),
            incoming_bi: bi_streams.fuse(),
            incoming_datagrams: datagrams.fuse(),
            service,
            active_peers,
        }
    }

    pub async fn start(mut self) {
        info!(peer =% self.connection.peer_id(), "InboundRequestHandler started");

        let mut inflight_requests = FuturesUnordered::new();

        loop {
            futures::select! {
                // anemo does not currently use uni streams so we can
                // just ignore and drop the stream
                uni = self.incoming_uni.select_next_some() => {
                    match uni {
                        Ok(recv_stream) => trace!("incoming uni stream! {}", recv_stream.id()),
                        Err(e) => {
                            warn!("error listening for incoming uni streams: {e}");
                            break;
                        }
                    }
                },
                bi = self.incoming_bi.select_next_some() => {
                    match bi {
                        Ok((bi_tx, bi_rx)) => {
                            info!("incoming bi stream! {}", bi_tx.id());
                            let request_handler =
                                BiStreamRequestHandler::new(self.connection.peer_id(), self.service.clone(), bi_tx, bi_rx);
                            inflight_requests.push(request_handler.handle());
                        }
                        Err(e) => {
                            warn!("error listening for incoming bi streams: {e}");
                            break;
                        }
                    }
                },
                // anemo does not currently use datagrams so we can
                // just ignore them
                datagram = self.incoming_datagrams.select_next_some() => {
                    match datagram {
                        Ok(datagram) => trace!("incoming datagram of length: {}", datagram.len()),
                        Err(e) => {
                            warn!("error listening for datagrams: {e}");
                            break;
                        }
                    }
                },
                () = inflight_requests.select_next_some() => {},
                complete => break,
            }
        }

        self.active_peers.remove_with_stable_id(
            self.connection.peer_id(),
            self.connection.stable_id(),
            crate::types::DisconnectReason::ConnectionLost,
        );

        info!("InboundRequestHandler ended");
    }
}

struct BiStreamRequestHandler {
    peer_id: PeerId,
    service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
    send_stream: FramedWrite<SendStream, LengthDelimitedCodec>,
    recv_stream: FramedRead<RecvStream, LengthDelimitedCodec>,
}

impl BiStreamRequestHandler {
    fn new(
        peer_id: PeerId,
        service: BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>,
        send_stream: SendStream,
        recv_stream: RecvStream,
    ) -> Self {
        Self {
            peer_id,
            service,
            send_stream: FramedWrite::new(send_stream, network_message_frame_codec()),
            recv_stream: FramedRead::new(recv_stream, network_message_frame_codec()),
        }
    }

    async fn handle(self) {
        if let Err(e) = self.do_handle().await {
            warn!("handling request failed: {e}");
        }
    }

    async fn do_handle(mut self) -> Result<()> {
        //
        // Read Request
        //

        let mut request = read_request(&mut self.recv_stream).await?;

        // Set the PeerId of this peer
        request.extensions_mut().insert(self.peer_id);

        // Issue request to configured Service
        let response = self.service.oneshot(request).await.expect("Infallible");

        //
        // Write Response
        //

        write_response(&mut self.send_stream, response).await?;
        self.send_stream.get_mut().finish().await?;

        Ok(())
    }
}
