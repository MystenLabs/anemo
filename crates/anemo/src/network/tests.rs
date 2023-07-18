use crate::{types::PeerEvent, Network, NetworkRef, Request, Response, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::FutureExt;
use std::{convert::Infallible, time::Duration};
use tower::{util::BoxCloneService, ServiceExt};
use tracing::trace;

#[tokio::test]
async fn basic_network() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let msg = b"The Way of Kings";

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let peer = network_1.connect(network_2.local_addr()).await?;
    let response = network_1
        .rpc(peer, Request::new(msg.as_ref().into()))
        .await?;
    assert_eq!(response.into_body(), msg.as_ref());

    let msg = b"Words of Radiance";
    let peer_id_1 = network_1.peer_id();
    let response = network_2
        .rpc(peer_id_1, Request::new(msg.as_ref().into()))
        .await?;
    assert_eq!(response.into_body(), msg.as_ref());
    Ok(())
}

#[tokio::test]
async fn connect() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let peer = network_1.connect(network_2.local_addr()).await?;
    assert_eq!(peer, network_2.peer_id());

    Ok(())
}

#[tokio::test]
async fn connect_with_peer_id() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let peer = network_1
        .connect_with_peer_id(network_2.local_addr(), network_2.peer_id())
        .await?;
    assert_eq!(peer, network_2.peer_id());

    Ok(())
}

#[tokio::test]
async fn connect_with_invalid_peer_id() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;
    let network_3 = build_network()?;

    // Try to dial network 2, but with network 3's peer id
    network_1
        .connect_with_peer_id(network_2.local_addr(), network_3.peer_id())
        .await
        .unwrap_err();

    Ok(())
}

#[tokio::test]
async fn connect_with_invalid_peer_id_ensure_server_doesnt_succeed() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;
    let network_3 = build_network()?;

    let (mut subscriber_2, _) = network_2.subscribe().unwrap();

    // Try to dial network 2, but with network 3's peer id
    network_1
        .connect_with_peer_id(network_2.local_addr(), network_3.peer_id())
        .await
        .unwrap_err();

    tokio::task::yield_now().await;

    // network 2 dialing 3 should succeed
    network_2
        .connect_with_peer_id(network_3.local_addr(), network_3.peer_id())
        .await
        .unwrap();

    assert_eq!(
        subscriber_2.try_recv(),
        Ok(PeerEvent::NewPeer(network_3.peer_id()))
    );

    drop(network_2);

    assert_eq!(
        subscriber_2.recv().await,
        Ok(PeerEvent::LostPeer(
            network_3.peer_id(),
            crate::types::DisconnectReason::LocallyClosed
        )),
    );
    assert_eq!(
        subscriber_2.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Closed),
    );

    Ok(())
}

#[tokio::test]
async fn connect_with_hostname() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;
    let network_3 = build_network()?;

    let peer = network_1
        .connect_with_peer_id(
            ("localhost", network_2.local_addr().port()),
            network_2.peer_id(),
        )
        .await?;
    assert_eq!(peer, network_2.peer_id());

    let peer = network_1
        .connect_with_peer_id(
            format!("localhost:{}", network_3.local_addr().port()),
            network_3.peer_id(),
        )
        .await?;
    assert_eq!(peer, network_3.peer_id());

    Ok(())
}

#[tokio::test]
async fn max_concurrent_connections_0() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    // Setup a network which disallows all incoming connections
    let config = crate::Config {
        max_concurrent_connections: Some(0),
        ..Default::default()
    };
    let network_1 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test")
        .config(config)
        .start(echo_service())?;

    let network_2 = build_network()?;

    network_2
        .connect_with_peer_id(network_1.local_addr(), network_1.peer_id())
        .await
        .unwrap_err();

    Ok(())
}

#[tokio::test]
async fn max_concurrent_connections_1() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    // Setup a network which disallows all incoming connections
    let config = crate::Config {
        max_concurrent_connections: Some(1),
        ..Default::default()
    };
    let network_1 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test")
        .config(config)
        .start(echo_service())?;

    let network_2 = build_network()?;
    let network_3 = build_network()?;

    // first connection succeeds
    network_2
        .connect_with_peer_id(network_1.local_addr(), network_1.peer_id())
        .await
        .unwrap();

    // second connection fails
    network_3
        .connect_with_peer_id(network_1.local_addr(), network_1.peer_id())
        .await
        .unwrap_err();

    // explicitly making an outbound connection bypasses this limit
    network_1
        .connect_with_peer_id(network_3.local_addr(), network_3.peer_id())
        .await
        .unwrap();

    Ok(())
}

#[tokio::test]
async fn reject_peer_with_affinity_never() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    // Configure peer 2 with affinity never
    let peer_info_2 = crate::types::PeerInfo {
        peer_id: network_2.peer_id(),
        affinity: crate::types::PeerAffinity::Never,
        address: vec![],
    };
    network_1.known_peers().insert(peer_info_2);

    // When peer 2 tries to connect peer 1 will reject it
    network_2
        .connect_with_peer_id(network_1.local_addr(), network_1.peer_id())
        .await
        .unwrap_err();

    Ok(())
}

#[tokio::test]
async fn peers_with_affinity_never_are_not_dialed_in_the_background() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;
    let network_3 = build_network()?;
    let network_4 = build_network()?;

    let mut subscriber_1 = network_1.subscribe()?.0;

    // Configure peer 2 with affinity never
    let peer_info_2 = crate::types::PeerInfo {
        peer_id: network_2.peer_id(),
        affinity: crate::types::PeerAffinity::Never,
        address: vec![],
    };
    network_1.known_peers().insert(peer_info_2);
    // Configure peer 3 with high affinity
    let peer_info_3 = crate::types::PeerInfo {
        peer_id: network_3.peer_id(),
        affinity: crate::types::PeerAffinity::High,
        address: vec![network_3.local_addr().into()],
    };
    network_1.known_peers().insert(peer_info_3);
    // Configure peer 4 with Allowed affinity
    let peer_info_4 = crate::types::PeerInfo {
        peer_id: network_4.peer_id(),
        affinity: crate::types::PeerAffinity::Allowed,
        address: vec![network_4.local_addr().into()],
    };
    network_1.known_peers().insert(peer_info_4);

    // When peer 2 tries to connect peer 1 will reject it
    network_2
        .connect_with_peer_id(network_1.local_addr(), network_1.peer_id())
        .await
        .unwrap_err();

    // We only ever see connections being made/lost with peer 3 and not peer 2 or 4
    let peer_id_3 = network_3.peer_id();
    assert_eq!(PeerEvent::NewPeer(peer_id_3), subscriber_1.recv().await?);

    drop(network_3);

    assert_eq!(
        PeerEvent::LostPeer(peer_id_3, crate::types::DisconnectReason::ApplicationClosed),
        subscriber_1.recv().await?
    );

    Ok(())
}

fn build_network() -> Result<Network> {
    build_network_with_addr("localhost:0")
}

fn build_network_with_addr(addr: &str) -> Result<Network> {
    let network = Network::bind(addr)
        .random_private_key()
        .server_name("test")
        .start(echo_service())?;

    trace!(
        address =% network.local_addr(),
        peer_id =% network.peer_id(),
        "starting network"
    );

    Ok(network)
}

fn echo_service() -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
    let handle = move |request: Request<Bytes>| async move {
        trace!("received: {}", request.body().escape_ascii());
        let response = Response::new(request.into_body());
        Result::<Response<Bytes>, Infallible>::Ok(response)
    };

    tower::service_fn(handle).boxed_clone()
}

#[tokio::test]
async fn ip6_calling_ip4() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network_with_addr("[::]:0")?;
    let network_2 = build_network_with_addr("127.0.0.1:0")?;

    let msg = b"The Way of Kings";
    let peer = network_1.connect(network_2.local_addr()).await?;
    let response = network_1
        .rpc(peer, Request::new(msg.as_ref().into()))
        .await?;

    println!("{}", response.body().escape_ascii());

    Ok(())
}

#[tokio::test]
async fn localhost_calling_anyaddr() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network_with_addr("0.0.0.0:0")?;
    let network_2 = build_network_with_addr("127.0.0.1:0")?;

    let msg = b"The Way of Kings";
    let peer = network_2
        .connect((std::net::Ipv4Addr::LOCALHOST, network_1.local_addr().port()))
        .await?;

    let response = network_2
        .rpc(peer, Request::new(msg.as_ref().into()))
        .await?;

    println!("{}", response.body().escape_ascii());

    let response = network_1
        .rpc(network_2.peer_id(), Request::new(msg.as_ref().into()))
        .await?;

    println!("{}", response.body().escape_ascii());

    Ok(())
}

#[tokio::test]
async fn dropped_connection() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let msg = b"The Way of Kings";
    let peer = network_1.connect(network_2.local_addr()).await?;
    let response = network_1
        .rpc(peer, Request::new(msg.as_ref().into()))
        .await?;

    println!("{}", response.body().escape_ascii());

    let mut peer = network_1.peer(peer).unwrap();

    drop(network_2);

    peer.rpc(Request::new(msg.as_ref().into()))
        .await
        .unwrap_err();

    Ok(())
}

#[tokio::test]
async fn basic_connectivity_check() -> Result<()> {
    use crate::types::{DisconnectReason, PeerEvent::*};

    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let peer_id_1 = network_1.peer_id();
    let peer_id_2 = network_2.peer_id();

    let peer_info_2 = crate::types::PeerInfo {
        peer_id: peer_id_2,
        affinity: crate::types::PeerAffinity::High,
        address: vec![network_2.local_addr().into()],
    };
    let mut subscriber_1 = network_1.subscribe()?.0;
    let mut subscriber_2 = network_2.subscribe()?.0;

    network_1.known_peers().insert(peer_info_2);

    assert_eq!(NewPeer(peer_id_2), subscriber_1.recv().await?);
    assert_eq!(NewPeer(peer_id_1), subscriber_2.recv().await?);

    network_1.known_peers().remove(&peer_id_2).unwrap();
    network_1.disconnect(peer_id_2)?;

    assert_eq!(
        LostPeer(peer_id_2, DisconnectReason::Requested),
        subscriber_1.recv().await?
    );
    assert_eq!(
        LostPeer(peer_id_1, DisconnectReason::ApplicationClosed),
        subscriber_2.recv().await?
    );

    Ok(())
}

#[tokio::test]
async fn basic_connectivity_check_with_allowlist_affinity() -> Result<()> {
    use crate::types::PeerEvent::*;

    let _guard = crate::init_tracing_for_testing();

    let network_1 = build_network()?;
    let network_2 = build_network()?;

    let peer_id_1 = network_1.peer_id();
    let peer_id_2 = network_2.peer_id();

    let mut subscriber_1 = network_1.subscribe()?.0;
    let mut subscriber_2 = network_2.subscribe()?.0;

    network_1.known_peers().insert(crate::types::PeerInfo {
        peer_id: peer_id_2,
        affinity: crate::types::PeerAffinity::Allowed,
        address: vec![network_2.local_addr().into()],
    });

    network_2.known_peers().insert(crate::types::PeerInfo {
        peer_id: peer_id_1,
        affinity: crate::types::PeerAffinity::Allowed,
        address: vec![network_1.local_addr().into()],
    });
    // Peers shouldn't connect each other.
    let mut timeout = tokio::time::sleep(Duration::from_secs(5)).boxed();
    tokio::select! {
        _ = subscriber_1.recv() => return Err(anyhow::anyhow!("peer 1 should not have received a peer event")),
        _ = subscriber_2.recv() => return Err(anyhow::anyhow!("peer 2 should not have received a peer event")),
        _ = &mut timeout => (),
    };

    // Now remove peer2 from network1 and add it back as High affinity.
    network_1.known_peers().remove(&peer_id_2).unwrap();
    network_1.known_peers().insert(crate::types::PeerInfo {
        peer_id: peer_id_2,
        affinity: crate::types::PeerAffinity::High,
        address: vec![network_2.local_addr().into()],
    });

    // Expect both to have new peer events.
    assert_eq!(NewPeer(peer_id_2), subscriber_1.recv().await?);
    assert_eq!(NewPeer(peer_id_1), subscriber_2.recv().await?);

    Ok(())
}

#[tokio::test]
async fn test_network_isolation() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();

    let network_1 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test1")
        .start(echo_service())?;
    let network_2 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test2")
        .start(echo_service())?;
    let network_3 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test2")
        .start(echo_service())?;

    assert!(network_2.connect(network_1.local_addr()).await.is_err());
    assert!(network_1.connect(network_2.local_addr()).await.is_err());
    assert!(network_2.connect(network_3.local_addr()).await.is_ok());
    assert!(network_2.connect(network_2.local_addr()).await.is_ok());

    let network_4 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test3")
        .alternate_server_name("test3dot1")
        .start(echo_service())?;
    let network_5 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test3")
        .start(echo_service())?;
    let network_6 = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test3dot1")
        .start(echo_service())?;

    // network_4 and network_5 talk to each other with "test3"
    assert!(network_4.connect(network_5.local_addr()).await.is_ok());
    assert!(network_5.connect(network_4.local_addr()).await.is_ok());

    // network_4 accepts "test3dot1" as server but can't init connection to network_6 as client
    assert!(network_4.connect(network_6.local_addr()).await.is_err());
    assert!(network_6.connect(network_4.local_addr()).await.is_ok());

    // network_5 and network_6 can't talk to each other
    assert!(network_5.connect(network_6.local_addr()).await.is_err());
    assert!(network_6.connect(network_5.local_addr()).await.is_err());

    Ok(())
}

// Ensure that when all Network handles are dropped that the network is shutdown
#[tokio::test]
async fn drop_shutdown() -> Result<()> {
    use tokio::sync::mpsc::error::TryRecvError;

    let _guard = crate::init_tracing_for_testing();

    let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);

    let service = {
        let handle = move |request: Request<Bytes>| {
            let sender = sender.clone();
            async move {
                let _sender = sender;
                let response = Response::new(request.into_body());
                Result::<Response<Bytes>, Infallible>::Ok(response)
            }
        };

        tower::service_fn(handle)
    };

    let network = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test")
        .start(service)?;

    let network_2 = build_network()?;

    let peer = network_2.connect(network.local_addr()).await?;
    let _response = network_2.rpc(peer, Request::new(Bytes::new())).await?;

    assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());

    let network_ref = network.downgrade();

    // Just check to see if upgrade is successful
    assert!(network_ref.upgrade().is_some());

    drop(network);

    // Now network upgrading should fail
    assert!(network_ref.upgrade().is_none());

    // And the network should eventually be completely stopped
    assert_eq!(None, receiver.recv().await);
    assert_eq!(Err(TryRecvError::Disconnected), receiver.try_recv());

    let err = network_2
        .rpc(peer, Request::new(Bytes::new()))
        .await
        .unwrap_err();

    tracing::info!("err: {err}");

    Ok(())
}

#[tokio::test]
async fn explicit_shutdown() -> Result<()> {
    use tokio::sync::mpsc::error::TryRecvError;

    let _guard = crate::init_tracing_for_testing();

    let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);

    let service = {
        let handle = move |request: Request<Bytes>| {
            let sender = sender.clone();
            async move {
                let _sender = sender;
                let response = Response::new(request.into_body());
                Result::<Response<Bytes>, Infallible>::Ok(response)
            }
        };

        tower::service_fn(handle)
    };

    let network = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test")
        .start(service)?;

    let network_2 = build_network()?;

    let peer = network_2.connect(network.local_addr()).await?;
    let _response = network_2.rpc(peer, Request::new(Bytes::new())).await?;

    assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());

    let network_ref = network.downgrade();

    // Just check to see if upgrade is successful
    assert!(network_ref.upgrade().is_some());

    // shutdown the network
    network.shutdown().await?;

    // Now network upgrading should fail
    assert!(network_ref.upgrade().is_none());

    // And all clones of the service should have been dropped by now
    assert_eq!(Err(TryRecvError::Disconnected), receiver.try_recv());

    let err = network_2
        .rpc(peer, Request::new(Bytes::new()))
        .await
        .unwrap_err();

    tracing::info!("err: {err}");

    Ok(())
}

#[tokio::test]
async fn subscribe_channel_closes_on_shutdown() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();
    let network = build_network()?;
    let mut subscriber = network.subscribe()?.0;

    drop(network);

    assert_eq!(
        Err(tokio::sync::broadcast::error::RecvError::Closed),
        subscriber.recv().await
    );

    Ok(())
}

#[tokio::test]
async fn subscribe_channel_closes_on_explicit_shutdown() -> Result<()> {
    let _guard = crate::init_tracing_for_testing();
    let network = build_network()?;
    let mut subscriber = network.subscribe()?.0;

    network.shutdown().await?;

    assert_eq!(
        Err(tokio::sync::broadcast::error::TryRecvError::Closed),
        subscriber.try_recv(),
    );

    assert!(network.subscribe().is_err());

    Ok(())
}

// Test to verify that anemo will perform an early termination of a request handler in the event
// that the requesting side terminated the RPC prematurely, perhaps due to a timeout.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn early_termination_of_request_handlers() {
    use std::time::Duration;
    use tokio::{
        sync::{mpsc, oneshot},
        time::timeout,
    };

    const HANDLER_SLEEP: u64 = 100;

    let _guard = crate::init_tracing_for_testing();

    let (sender, mut receiver) = mpsc::channel::<oneshot::Receiver<()>>(1);

    let service = {
        let handle = move |request: Request<Bytes>| {
            let sender = sender.clone();
            async move {
                let (_sender, receiver) = oneshot::channel();
                sender.send(receiver).await.unwrap();
                tokio::time::sleep(Duration::from_secs(HANDLER_SLEEP)).await;
                let response = Response::new(request.into_body());
                Result::<Response<Bytes>, Infallible>::Ok(response)
            }
        };

        tower::service_fn(handle)
    };

    let network = Network::bind("localhost:0")
        .random_private_key()
        .server_name("test")
        .start(service)
        .unwrap();

    let network_2 = build_network().unwrap();

    let peer = network_2.connect(network.local_addr()).await.unwrap();

    let client_fut = async {
        timeout(
            Duration::from_secs(1),
            network_2.rpc(peer, Request::new(Bytes::new())),
        )
        .await
        .unwrap_err();
    };

    let server_fut = async {
        use tokio::sync::oneshot::error::TryRecvError;

        let mut receiver = receiver.recv().await.unwrap();
        assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(Err(TryRecvError::Closed), receiver.try_recv());
    };

    futures::future::join(client_fut, server_fut).await;
}

#[tokio::test]
async fn user_provided_client_service_layer() {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll},
    };
    use tower::{
        layer::{layer_fn, Layer},
        Service,
    };

    #[derive(Clone)]
    pub struct CounterService<S> {
        counter: Arc<AtomicUsize>,
        service: S,
    }

    impl<S, Request> Service<Request> for CounterService<S>
    where
        S: Service<Request>,
    {
        type Response = S::Response;
        type Error = S::Error;
        type Future = S::Future;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.service.poll_ready(cx)
        }

        fn call(&mut self, request: Request) -> Self::Future {
            // Increment the count
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            self.service.call(request)
        }
    }

    let create_network = || {
        let server_counter = Arc::new(AtomicUsize::new(0));
        let server_counter_clone = server_counter.clone();
        let server_layer = layer_fn(move |service| CounterService {
            service,
            counter: server_counter_clone.clone(),
        });
        let client_counter = Arc::new(AtomicUsize::new(0));
        let client_counter_clone = client_counter.clone();
        let client_layer = layer_fn(move |service| CounterService {
            service,
            counter: client_counter_clone.clone(),
        });

        (
            Network::bind("localhost:0")
                .server_name("test")
                .outbound_request_layer(client_layer)
                .random_private_key()
                .start(server_layer.layer(echo_service()))
                .unwrap(),
            server_counter,
            client_counter,
        )
    };

    let _guard = crate::init_tracing_for_testing();

    let (network_1, server_counter_1, client_counter_1) = create_network();
    let (network_2, server_counter_2, client_counter_2) = create_network();

    let peer_id = network_1.connect(network_2.local_addr()).await.unwrap();

    let request = Request::new(Bytes::from_static(b"hello"));
    let _ = network_1
        .peer(peer_id)
        .unwrap()
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(0, server_counter_1.load(Ordering::SeqCst));
    assert_eq!(1, client_counter_1.load(Ordering::SeqCst));
    assert_eq!(1, server_counter_2.load(Ordering::SeqCst));
    assert_eq!(0, client_counter_2.load(Ordering::SeqCst));

    let request = Request::new(Bytes::from_static(b"hello"));
    let _ = network_2
        .peer(network_1.peer_id())
        .unwrap()
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(1, server_counter_1.load(Ordering::SeqCst));
    assert_eq!(1, client_counter_1.load(Ordering::SeqCst));
    assert_eq!(1, server_counter_2.load(Ordering::SeqCst));
    assert_eq!(1, client_counter_2.load(Ordering::SeqCst));
}

// Verify that we properly include a `NetworkRef` as an extension to request handlers
#[tokio::test]
async fn network_ref_via_extension() -> Result<()> {
    let svc = tower::service_fn(|req: Request<Bytes>| async move {
        let network_ref = req.extensions().get::<NetworkRef>().unwrap();
        let count = network_ref.upgrade().unwrap().peers().len();
        let mut buf = BytesMut::new();
        buf.put_u8(count as u8);
        Ok::<_, Infallible>(Response::new(buf.freeze()))
    });

    let network_1 = Network::bind("localhost:0")
        .server_name("test")
        .random_private_key()
        .start(svc)?;
    let network_2 = build_network()?;

    let peer = network_2.connect(network_1.local_addr()).await?;
    let mut response = network_2
        .rpc(peer, Request::new(Bytes::new()))
        .await?
        .into_inner();

    assert_eq!(1, response.get_u8());

    Ok(())
}
