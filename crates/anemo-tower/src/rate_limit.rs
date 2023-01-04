//! Middleware that adds a per-peer rate limit to inbound requests.
//!
//! # Example
//!
//! ```
//! use anemo_tower::rate_limit::RateLimitLayer;
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
//!     .layer(RateLimitLayer::new(governor::Quota::per_second(nonzero!(1u32))))
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

/// [`Layer`] for adding a per-peer rate limit to inbound requests.
///
/// See the [module docs](super::rate_limit) for more details.
#[derive(Clone, Debug)]
pub struct RateLimitLayer {
    pub limiter: SharedRateLimiter,
}

impl RateLimitLayer {
    /// Creates a new [`RateLimitLayer`].
    pub fn new(quota: governor::Quota) -> Self {
        RateLimitLayer {
            limiter: Arc::new(RateLimiter::keyed(quota)),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimit {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

/// Middleware for adding a per-peer rate limit to inbound requests.
///
/// See the [module docs](super::rate_limit) for more details.
#[derive(Clone, Debug)]
pub struct RateLimit<S> {
    pub inner: S,
    pub limiter: SharedRateLimiter,
}

impl<S> RateLimit<S> {
    /// Creates a new [`RateLimit`].
    pub fn new(inner: S, quota: governor::Quota) -> Self {
        Self {
            limiter: Arc::new(RateLimiter::keyed(quota)),
            inner,
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
    pub fn layer(quota: governor::Quota) -> RateLimitLayer {
        RateLimitLayer {
            limiter: Arc::new(RateLimiter::keyed(quota)),
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
        let mut inner = self.inner.clone();

        let fut = async move {
            let peer_id = req.peer_id().ok_or_else(|| {
                anemo::rpc::Status::internal("rate limiter missing request PeerId")
            })?;
            limiter.until_key_ready(peer_id).await;
            inner.call(req).await
        };
        Box::pin(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::RateLimitLayer;
    use anemo::{Request, Response};
    use bytes::Bytes;
    use nonzero_ext::nonzero;
    use std::time::Duration;
    use tower::{ServiceBuilder, ServiceExt};

    #[tokio::test]
    async fn basic() {
        let service_fn = tower::service_fn(|_req: Request<Bytes>| async move {
            Ok::<_, anemo::rpc::Status>(Response::new(Bytes::new()))
        });

        let peer = anemo::PeerId([0; 32]);

        let layer = RateLimitLayer::new(governor::Quota::per_hour(nonzero!(1u32)));

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
}
