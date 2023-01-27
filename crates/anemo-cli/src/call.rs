use crate::{util, Config};
use anemo::Network;
use anemo_tower::trace::TraceLayer;

pub async fn run(
    config: Config,
    address: Box<str>,
    server_name: String,
    service_name: String,
    method_name: String,
    request: String,
) {
    let network = Network::bind("localhost:0")
        .private_key(util::random_key())
        .server_name(server_name)
        .outbound_request_layer(TraceLayer::new_for_client_and_server_errors())
        .start(tower::service_fn(util::noop_handle))
        .unwrap();

    let peer_id = match network.connect(address).await {
        Ok(peer_id) => peer_id,
        Err(e) => {
            println!("error connecting: {e:?}");
            return;
        }
    };

    let peer = network.peer(peer_id).expect("just-connected peer is found");

    let method_fn = config
        .service_map
        .get(&service_name)
        .expect("service is configured")
        .method_map
        .get(&method_name)
        .expect("method is configured");
    let result = (method_fn)(peer, request).await;
    println!("{result}");

    // Explicitly disconnect to avoid error:
    // "ActivePeers should be empty after all connection handlers have terminated"
    let _result = network.disconnect(peer_id); // no problem if disconnect fails
}
