use crate::util;

pub async fn run(address: &str, server_name: &str) {
    match util::create_client_network(address, server_name).await {
        Ok((_network, peer)) => {
            println!(
                "successfully connected to peer with ID: {:?}",
                peer.peer_id()
            )
        }
        Err(e) => {
            println!("connection error: {e:?}");
        }
    };
}
