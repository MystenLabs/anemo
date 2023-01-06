//! Middleware that adds a per-peer inflight limit to inbound requests.
//!
//! # Example
//!
//! ```
//! use anemo_tower::inflight_limit::{InflightLimitLayer, WaitMode};
//! use anemo::{Request, Response};
//! use tower::{Service, ServiceExt, ServiceBuilder, service_fn};
//! use bytes::Bytes;
//!
//! async fn handle(req: Request<Bytes>) -> Result<Response<Bytes>, anemo::rpc::Status> {
//!     Ok(Response::new(Bytes::new()))
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), anemo::rpc::Status> {
//! let mut service = ServiceBuilder::new()
//!     .layer(InflightLimitLayer::new(3, WaitMode::ReturnError))
//!     .service_fn(handle);
//!
//! // Set fake PeerId.
//! let request = Request::new(Bytes::new()).with_extension(anemo::PeerId([0; 32]));
//!
//! // Call the service.
//! let response = service
//!     .ready()
//!     .await?
//!     .call(request)
//!     .await?;
//! # Ok(())
//! # }
//! ```

use anemo::{types::response::StatusCode, PeerId, Request, Response};
use dashmap::DashMap;
use futures::future::BoxFuture;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::Semaphore;
use tower::{layer::Layer, Service};
use tracing::debug;

/// What to do if inflight limit is exceeded.
#[derive(Clone, Copy, Debug)]
pub enum WaitMode {
    /// Blocks request until it can be serviced.
    Block,
    /// Returns an error indicating the minimum wait time required to try again.
    ReturnError,
}

/// [`Layer`] for adding a per-peer inflight limit to inbound requests.
///
/// See the [module docs](super::inflight_limit) for more details.
#[derive(Clone, Debug)]
pub struct InflightLimitLayer {
    inflight: Arc<DashMap<PeerId, Arc<Semaphore>>>,
    max_inflight: usize,
    wait_mode: WaitMode,
}

impl InflightLimitLayer {
    /// Create a new [`InflightLimitLayer`].
    pub fn new(max_inflight: usize, wait_mode: WaitMode) -> Self {
        InflightLimitLayer {
            inflight: Arc::new(DashMap::new()),
            max_inflight,
            wait_mode,
        }
    }
}

impl<S> Layer<S> for InflightLimitLayer {
    type Service = InflightLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        InflightLimit {
            inner,
            inflight: self.inflight.clone(),
            max_inflight: self.max_inflight,
            wait_mode: self.wait_mode,
        }
    }
}

/// Middleware for adding a per-peer inflight limit to inbound requests.
///
/// See the [module docs](super::inflight_limit) for more details.
#[derive(Clone, Debug)]
pub struct InflightLimit<S> {
    inner: S,
    // TODO: Garbage collect old PeerIds.
    inflight: Arc<DashMap<PeerId, Arc<Semaphore>>>,
    max_inflight: usize,
    wait_mode: WaitMode,
}

impl<S> InflightLimit<S> {
    /// Create a new [`InflightLimit`].
    pub fn new(inner: S, max_inflight: usize, wait_mode: WaitMode) -> Self {
        Self {
            inner,
            inflight: Arc::new(DashMap::new()),
            max_inflight,
            wait_mode,
        }
    }

    /// Gets a reference to the underlying service.
    pub fn inner_ref(&self) -> &S {
        &self.inner
    }

    /// Gets a mutable reference to the underlying service.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes `self`, returning the underlying service.
    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Returns a new [`Layer`] that wraps services with a `InflightLimit` middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(max_inflight: usize, wait_mode: WaitMode) -> InflightLimitLayer {
        InflightLimitLayer {
            inflight: Arc::new(DashMap::new()),
            max_inflight,
            wait_mode,
        }
    }
}

impl<ResBody, ReqBody, S> Service<Request<ReqBody>> for InflightLimit<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>, Error = anemo::rpc::Status>
        + 'static
        + Clone
        + Send,
    <S as Service<Request<ReqBody>>>::Future: Send,
    ReqBody: 'static + Send + Sync,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let inflight = self.inflight.clone();
        let max_inflight = self.max_inflight;
        let wait_mode = self.wait_mode;
        let mut inner = self.inner.clone();

        let fut = async move {
            let peer_id = req.peer_id().ok_or_else(|| {
                anemo::rpc::Status::internal("inflight limiter missing request PeerId")
            })?;
            let semaphore = {
                let semaphore_entry = inflight
                    .entry(*peer_id)
                    .or_insert_with(|| Arc::new(Semaphore::new(max_inflight)));
                semaphore_entry.value().clone()
            };
            let _permit = match wait_mode {
                WaitMode::Block => semaphore.acquire().await.map_err(|e| {
                    anemo::rpc::Status::internal(format!(
                        "failed to acquire inflight limiter permit: {e:?}"
                    ))
                })?,
                WaitMode::ReturnError => semaphore.try_acquire().map_err(|e| match e {
                    tokio::sync::TryAcquireError::Closed => {
                        anemo::rpc::Status::new(StatusCode::InternalServerError)
                    }
                    tokio::sync::TryAcquireError::NoPermits => {
                        anemo::rpc::Status::new(StatusCode::TooManyRequests)
                    }
                })?,
            };
            debug!("acquired inflight limiter permit for peer {peer_id:?}");
            inner.call(req).await
        };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::{InflightLimitLayer, WaitMode};
    use anemo::{types::response::StatusCode, Request, Response};
    use bytes::Bytes;
    use std::time::Duration;
    use tower::{service_fn, ServiceBuilder, ServiceExt};

    #[tokio::test]
    async fn block() {
        let working_service = service_fn(|_req: Request<Bytes>| async move {
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });
        let waiting_service = service_fn(|_req: Request<Bytes>| async move {
            tokio::time::sleep(Duration::from_secs(86400)).await;
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });

        let peer = anemo::PeerId([0; 32]);

        // Verify we get a result for working service.
        let svc = ServiceBuilder::new()
            .layer(InflightLimitLayer::new(1, WaitMode::Block))
            .service(working_service);
        let request = Request::new(Bytes::new()).with_extension(peer);
        svc.oneshot(request).await.unwrap();

        // Verify waiting service blocks.
        let layer = InflightLimitLayer::new(1, WaitMode::Block);
        let working_svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(working_service);
        let waiting_svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(waiting_service);

        let request = Request::new(Bytes::new()).with_extension(peer);
        tokio::task::spawn(waiting_svc.oneshot(request));
        tokio::time::sleep(Duration::from_millis(100)).await; // let waiting service start
        let request = Request::new(Bytes::new()).with_extension(peer);
        let timeout_resp =
            tokio::time::timeout(Duration::from_secs(2), working_svc.oneshot(request)).await;
        assert!(timeout_resp.is_err()); // working request should be blocked behind waiting request
    }

    #[tokio::test]
    async fn return_error() {
        let working_service = service_fn(|_req: Request<Bytes>| async move {
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });
        let waiting_service = service_fn(|_req: Request<Bytes>| async move {
            tokio::time::sleep(Duration::from_secs(86400)).await;
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });

        let peer = anemo::PeerId([0; 32]);

        // Verify waiting service returns error.
        let layer = InflightLimitLayer::new(1, WaitMode::ReturnError);
        let working_svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(working_service);
        let waiting_svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(waiting_service);

        let request = Request::new(Bytes::new()).with_extension(peer);
        tokio::task::spawn(waiting_svc.oneshot(request));
        tokio::time::sleep(Duration::from_millis(100)).await; // let waiting service start

        let request = Request::new(Bytes::new()).with_extension(peer);
        let response = working_svc.oneshot(request).await;
        assert_eq!(response.unwrap_err().status(), StatusCode::TooManyRequests);
    }
}
