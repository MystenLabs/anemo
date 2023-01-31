use crate::{util, Config};

pub async fn run(
    config: Config,
    address: &str,
    server_name: &str,
    service_name: &str,
    method_name: &str,
    request: String,
) {
    let (network, peer) = match util::create_client_network(address, server_name).await {
        Ok((network, peer)) => (network, peer),
        Err(e) => {
            println!("connection error: {e:?}");
            return;
        }
    };
    let peer_id = peer.peer_id();

    let method_fn = config
        .service_map
        .get(service_name)
        .expect("service is configured")
        .method_map
        .get(method_name)
        .expect("method is configured");
    let result = (method_fn)(peer, request).await;
    println!("{result}");

    // Explicitly disconnect to avoid error:
    // "ActivePeers should be empty after all connection handlers have terminated"
    let _result = network.disconnect(peer_id); // no problem if disconnect fails
}
