use greeter::{greeter_client::GreeterClient, greeter_server::Greeter};
use serde::{Deserialize, Serialize};

pub use anemo::rpc::codec::JsonCodec;
use anemo::{rpc::Status, Network, Request, Response};

use crate::greeter::greeter_server::GreeterServer;

mod greeter {
    include!(concat!(env!("OUT_DIR"), "/json.helloworld.Greeter.rs"));
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

#[anemo::codegen::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloResponse>, Status> {
        println!("Got a request from {:?}", request.peer_id());

        let reply = HelloResponse {
            message: format!("Hello {}!", request.into_body().name),
        };
        Ok(Response::new(reply))
    }

    async fn say_hello_2(&self, request: Request<HelloRequest>) -> Result<Response<()>, Status> {
        println!("Got a request from {:?}", request.peer_id());

        Ok(Response::new(()))
    }
}

#[tokio::main]
async fn main() {
    let mut rng = rand::thread_rng();
    let network_1 = Network::bind("localhost:0")
        .private_key({
            let mut bytes = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
            bytes
        })
        .server_name("test")
        .start(GreeterServer::new(MyGreeter::default()))
        .unwrap();

    let network_2 = Network::bind("localhost:0")
        .private_key({
            let mut bytes = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
            bytes
        })
        .server_name("test")
        .start(GreeterServer::new(MyGreeter::default()))
        .unwrap();

    let peer = network_1.connect(network_2.local_addr()).await.unwrap();

    let peer = network_1.peer(peer).unwrap();
    let mut client = GreeterClient::new(peer);
    dbg!(client
        .say_hello(HelloRequest {
            name: "Brandon".into()
        })
        .await
        .unwrap());

    let peer = network_2.peer(network_1.peer_id()).unwrap();
    let client = GreeterClient::new(peer);

    let mut handles = Vec::new();
    for _ in 0..50 {
        let mut client = client.clone();
        handles.push(async move {
            client
                .say_hello_2(HelloRequest {
                    name: "Ledger".into(),
                })
                .await
        });
    }

    dbg!(futures::future::join_all(handles).await);
}
