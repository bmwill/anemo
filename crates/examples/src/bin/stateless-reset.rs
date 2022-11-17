use anemo::{Request, Response};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::{convert::Infallible, time::Duration};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::info;

const KEY_1: [u8; 32] = [
    62, 178, 86, 143, 75, 93, 10, 229, 54, 166, 100, 220, 136, 72, 190, 52, 129, 161, 234, 239, 33,
    122, 104, 163, 153, 210, 71, 156, 138, 227, 37, 169,
];

const KEY_2: [u8; 32] = [
    191, 169, 214, 188, 251, 227, 56, 137, 88, 213, 151, 225, 31, 53, 232, 79, 170, 226, 169, 110,
    38, 191, 155, 21, 43, 80, 244, 167, 96, 221, 229, 13,
];

const ADDRESS_1: &str = "127.0.0.1:8080";
const ADDRESS_2: &str = "127.0.0.1:8081";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let (our_key, our_address, their_address) = if std::env::args().len() == 1 {
        (KEY_1, ADDRESS_1, ADDRESS_2)
    } else {
        (KEY_2, ADDRESS_2, ADDRESS_1)
    };

    let network = anemo::Network::bind(our_address)
        .private_key(our_key)
        .server_name("test")
        .start(echo_service())
        .unwrap();
    info!("address: {:?}", network.local_addr());

    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;

        let mut peers = network.peers();
        let peer_id = if peers.is_empty() {
            if std::env::args().len() != 1 {
                continue;
            }
            match network.connect(their_address).await {
                Ok(peer_id) => peer_id,
                Err(e) => {
                    info!("{e}");
                    continue;
                }
            }
        } else {
            peers.pop().unwrap()
        };

        let msg = Bytes::from_static(&[42; 128]);
        match network.rpc(peer_id, Request::new(msg)).await {
            Ok(_) => info!("success"),
            Err(e) => info!("{e}"),
        }
    }
}

fn echo_service() -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
    let handle = move |request: Request<Bytes>| async move {
        let response = Response::new(request.into_body());
        Result::<Response<Bytes>, Infallible>::Ok(response)
    };

    tower::service_fn(handle).boxed_clone()
}
