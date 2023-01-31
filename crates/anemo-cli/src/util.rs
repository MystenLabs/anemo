use std::convert::Infallible;

use anemo::{Network, Peer, Request, Response, Result};
use anemo_tower::trace::TraceLayer;
use bytes::Bytes;

async fn noop_handle(_request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
    Ok(Response::new(bytes::Bytes::new()))
}

pub async fn create_client_network(address: &str, server_name: &str) -> Result<(Network, Peer)> {
    let network = Network::bind("0.0.0.0:0")
        .private_key(random_key())
        .server_name(server_name)
        .outbound_request_layer(TraceLayer::new_for_client_and_server_errors())
        .start(tower::service_fn(noop_handle))
        .unwrap();
    let peer_id = network.connect(address).await?;
    let peer = network.peer(peer_id).expect("just-connected peer is found");
    Ok((network, peer))
}

pub fn random_key() -> [u8; 32] {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
    bytes
}
