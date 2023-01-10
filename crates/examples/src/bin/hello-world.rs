use anemo::{rpc::Status, types::PeerEvent, Network, Request, Response};
use anemo_tower::trace::TraceLayer;
use serde::{Deserialize, Serialize};
use tower::Layer;
use tracing::info;

use greeter::{
    greeter_client::GreeterClient,
    greeter_server::{Greeter, GreeterServer},
};
mod greeter {
    include!(concat!(env!("OUT_DIR"), "/example.helloworld.Greeter.rs"));
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HelloRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HelloResponse {
    pub message: String,
}

#[derive(Default)]
pub struct MyGreeter {}

#[anemo::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloResponse>, Status> {
        info!(
            "Got a request from {}",
            request.peer_id().unwrap().short_display(4)
        );

        let reply = HelloResponse {
            message: format!("Hello {}!", request.into_body().name),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let network_1 = Network::bind("localhost:0")
        .private_key(random_key())
        .server_name("test")
        .start(TraceLayer::new_for_server_errors().layer(GreeterServer::new(MyGreeter::default())))
        .unwrap();

    let network_2 = Network::bind("localhost:0")
        .private_key(random_key())
        .server_name("test")
        .outbound_request_layer(TraceLayer::new_for_client_and_server_errors())
        .start(GreeterServer::new(MyGreeter::default()))
        .unwrap();

    let network_2_addr = network_2.local_addr();

    let handle = tokio::spawn(async move {
        let (mut receiver, mut peers) = network_2.subscribe().unwrap();

        let peer_id = {
            if peers.is_empty() {
                match receiver.recv().await.unwrap() {
                    PeerEvent::NewPeer(peer_id) => peer_id,
                    PeerEvent::LostPeer(_, _) => todo!(),
                }
            } else {
                peers.pop().unwrap()
            }
        };

        let peer = network_2.peer(peer_id).unwrap();
        let client = GreeterClient::new(peer);

        let mut handles = Vec::new();
        for i in 0..2 {
            let mut client = client.clone();
            handles.push(async move {
                client
                    .say_hello(HelloRequest {
                        name: i.to_string(),
                    })
                    .await
                    .unwrap()
                    .into_inner()
            });
        }

        info!("{:#?}", futures::future::join_all(handles).await);
    });

    let peer = network_1.connect(network_2_addr).await.unwrap();

    let peer = network_1.peer(peer).unwrap();
    let mut client = GreeterClient::new(peer);
    let response = client
        .say_hello(HelloRequest {
            name: "Brandon".into(),
        })
        .await
        .unwrap();

    info!("{:#?}", response);

    handle.await.unwrap();
}

fn random_key() -> [u8; 32] {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
    bytes
}
