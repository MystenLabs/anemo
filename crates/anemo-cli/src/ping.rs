use crate::util;
use anemo::Network;
use anemo_tower::trace::TraceLayer;

pub async fn run(address: Box<str>, server_name: String) {
    let network = Network::bind("localhost:0")
        .private_key(util::random_key())
        .server_name(server_name)
        .outbound_request_layer(TraceLayer::new_for_client_and_server_errors())
        .start(tower::service_fn(util::noop_handle))
        .unwrap();

    match network.connect(address).await {
        Ok(peer_id) => println!("successfully connected to peer with ID: {peer_id:?}"),
        Err(e) => println!("error connecting: {e:?}"),
    }
}
