use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let network = anemo::Network::bind("0.0.0.0:0")
        .private_key(random_key())
        .server_name("sui")
        .start(anemo::Router::new())
        .unwrap();
    info!("address: {:?}", network.local_addr());

    let result = network
        // .connect("validator-0.devnet.sui.io:8084")
        // .connect("validator-2-udp.devnet.sui.io:8084")
        .connect("fullnode-udp.devnet.sui.io:8084")
        // .connect("fullnode-1.devnet.sui.io:8084")
        // .connect("fullnode-udp.staging.sui.io")
        // .connect("43.201.94.190:8082")
        // .connect("sea-suival-0.testnet.sui.io:8082")
        .await;
    info!("{:?}", result);
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
}

fn random_key() -> [u8; 32] {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
    bytes
}
