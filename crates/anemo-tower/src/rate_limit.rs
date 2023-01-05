//! Middleware that adds a per-peer rate limit to inbound requests.
//!
//! # Example
//!
//! ```
//! use anemo_tower::rate_limit::{RateLimitLayer, WaitMode};
//! use anemo::{Request, Response};
//! use bytes::Bytes;
//! use nonzero_ext::nonzero;
//! use tower::{Service, ServiceExt, ServiceBuilder, service_fn};
//!
//! async fn handle(req: Request<Bytes>) -> Result<Response<Bytes>, anemo::rpc::Status> {
//!     Ok(Response::new(Bytes::new()))
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), anemo::rpc::Status> {
//! // Example: rate limit of 1/s.
//! let mut service = ServiceBuilder::new()
//!     .layer(RateLimitLayer::new(governor::Quota::per_second(nonzero!(1u32)), WaitMode::Block))
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

use anemo::{Request, Response};
use futures::future::BoxFuture;
use governor::{
    clock::{Clock, DefaultClock},
    middleware::NoOpMiddleware,
    state::keyed::DefaultKeyedStateStore,
    RateLimiter,
};
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tower::{layer::Layer, Service};

type SharedRateLimiter = Arc<
    RateLimiter<
        anemo::PeerId,
        DefaultKeyedStateStore<anemo::PeerId>,
        DefaultClock,
        NoOpMiddleware<<DefaultClock as Clock>::Instant>,
    >,
>;

/// What to do if rate limit is exceeded.
#[derive(Clone, Copy, Debug)]
pub enum WaitMode {
    /// Blocks request until it can be serviced.
    Block,
    /// Returns an error indicating the minimum wait time required to try again.
    ReturnError,
}

/// Key for error response header indicating the minimum wait time before retry (in nanos).
pub const WAIT_NANOS_HEADER: &str = "wait-nanos";

/// [`Layer`] for adding a per-peer rate limit to inbound requests.
///
/// See the [module docs](super::rate_limit) for more details.
#[derive(Clone, Debug)]
pub struct RateLimitLayer {
    limiter: SharedRateLimiter,
    clock: DefaultClock,
    wait_mode: WaitMode,
}

impl RateLimitLayer {
    /// Creates a new [`RateLimitLayer`].
    pub fn new(quota: governor::Quota, wait_mode: WaitMode) -> Self {
        RateLimitLayer {
            limiter: Arc::new(RateLimiter::keyed(quota)),
            clock: DefaultClock::default(),
            wait_mode,
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimit {
            inner,
            limiter: self.limiter.clone(),
            clock: self.clock.clone(),
            wait_mode: self.wait_mode,
        }
    }
}

/// Middleware for adding a per-peer rate limit to inbound requests.
///
/// See the [module docs](super::rate_limit) for more details.
#[derive(Clone, Debug)]
pub struct RateLimit<S> {
    inner: S,
    limiter: SharedRateLimiter,
    clock: DefaultClock,
    wait_mode: WaitMode,
}

impl<S> RateLimit<S> {
    /// Creates a new [`RateLimit`]. If the quota is exceeded, behavior is determined by
    /// the indicated WaitMode.
    pub fn new(inner: S, quota: governor::Quota, wait_mode: WaitMode) -> Self {
        let clock = DefaultClock::default();
        Self {
            inner,
            limiter: Arc::new(RateLimiter::dashmap_with_clock(quota, &clock)),
            clock,
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

    /// Returns a new [`Layer`] that wraps services with a `RateLimit` middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(quota: governor::Quota, wait_mode: WaitMode) -> RateLimitLayer {
        let clock = DefaultClock::default();
        RateLimitLayer {
            limiter: Arc::new(RateLimiter::dashmap_with_clock(quota, &clock)),
            clock,
            wait_mode,
        }
    }
}

impl<ResBody, ReqBody, S> Service<Request<ReqBody>> for RateLimit<S>
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
        let limiter = self.limiter.clone();
        let clock = self.clock.clone();
        let wait_mode = self.wait_mode;
        let mut inner = self.inner.clone();

        let fut = async move {
            let peer_id = req.peer_id().ok_or_else(|| {
                anemo::rpc::Status::internal("rate limiter missing request PeerId")
            })?;
            match wait_mode {
                WaitMode::Block => limiter.until_key_ready(peer_id).await,
                WaitMode::ReturnError => {
                    if let Err(e) = limiter.check_key(peer_id) {
                        let wait_time = e.wait_time_from(clock.now());
                        return Err(anemo::rpc::Status::new(
                            anemo::types::response::StatusCode::TooManyRequests,
                        )
                        .with_header(WAIT_NANOS_HEADER, format!("{}", wait_time.as_nanos())));
                    }
                }
            };

            inner.call(req).await
        };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::{RateLimitLayer, WaitMode};
    use anemo::{Request, Response};
    use bytes::Bytes;
    use nonzero_ext::nonzero;
    use std::time::Duration;
    use tower::{ServiceBuilder, ServiceExt};

    #[tokio::test]
    async fn block() {
        let service_fn = tower::service_fn(|_req: Request<Bytes>| async move {
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });

        let peer = anemo::PeerId([0; 32]);

        let layer = RateLimitLayer::new(governor::Quota::per_hour(nonzero!(1u32)), WaitMode::Block);

        let svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(service_fn);
        let request = Request::new(Bytes::new()).with_extension(peer);
        svc.oneshot(request).await.unwrap();

        let svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(service_fn);
        let request = Request::new(Bytes::new()).with_extension(peer);
        let timeout_resp = tokio::time::timeout(Duration::from_secs(1), svc.oneshot(request)).await;
        assert!(timeout_resp.is_err()); // second request should be blocked on rate limit
    }

    #[tokio::test]
    async fn return_error() {
        let service_fn = tower::service_fn(|_req: Request<Bytes>| async move {
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });

        let peer = anemo::PeerId([0; 32]);

        let layer = RateLimitLayer::new(
            governor::Quota::per_hour(nonzero!(1u32)),
            WaitMode::ReturnError,
        );

        let svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(service_fn);
        let request = Request::new(Bytes::new()).with_extension(peer);
        svc.oneshot(request).await.unwrap();

        let svc = ServiceBuilder::new()
            .layer(layer.clone())
            .service(service_fn);
        let request = Request::new(Bytes::new()).with_extension(peer);
        let err_response = svc.oneshot(request).await.unwrap_err();
        let wait_nanos = err_response
            .headers()
            .get(super::WAIT_NANOS_HEADER)
            .unwrap()
            .parse::<u128>()
            .unwrap();
        assert!(wait_nanos > 3_540_000_000_000); // 59 minutes
    }
}
