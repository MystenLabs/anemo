mod config;
mod connection;
mod crypto;
mod endpoint;
mod error;
pub mod middleware;
mod network;
mod routing;
pub mod rpc;
pub mod types;

pub use config::{Config, QuicConfig};
pub use error::{Error, Result};
pub use network::{Builder, KnownPeers, Network, NetworkRef, Peer};
pub use routing::Router;
#[doc(inline)]
pub use types::{request::Request, response::Response, ConnectionOrigin, Direction, PeerId};

pub use async_trait::async_trait;

#[doc(hidden)]
pub mod codegen {
    pub use super::{
        error::BoxError, middleware::box_clone_layer::BoxCloneLayer, rpc::codec::Codec,
        types::response::IntoResponse, Request, Response,
    };
    pub use async_trait::async_trait;
    pub use bytes::Bytes;
    pub use futures::future::BoxFuture;
    pub use std::{
        future::Future,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
    };
    pub use tower::{
        layer::util::{Identity, Stack},
        util::BoxCloneService,
        Layer, Service,
    };

    pub type InboundRequestLayer<Req, Resp> = BoxCloneLayer<
        BoxCloneService<crate::Request<Req>, crate::Response<Resp>, crate::rpc::Status>,
        crate::Request<Req>,
        crate::Response<Resp>,
        crate::rpc::Status,
    >;
}

#[cfg(test)]
fn init_tracing_for_testing() -> ::tracing::dispatcher::DefaultGuard {
    use tracing_subscriber::{EnvFilter, FmtSubscriber};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,quinn=warn"));

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(filter)
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        // .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_test_writer()
        .finish();

    ::tracing::subscriber::set_default(subscriber)
}
