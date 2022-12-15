//! Set a header on the request.
//!
//! The header value to be set may be provided as a fixed value when the
//! middleware is constructed, or determined dynamically based on the request
//! by a closure. See the [`MakeHeaderValue`] trait for details.
//!
//! # Example
//!
//! Setting a header from a fixed value provided when the middleware is constructed:
//!
//! ```
//! use anemo::{Request, Response};
//! use anemo_tower::set_header::SetRequestHeaderLayer;
//! use tower::{Service, ServiceExt, ServiceBuilder};
//! use bytes::Bytes;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let client = tower::service_fn(|req: Request<Bytes>| async move {
//! #     let value = req.headers().get("User-Agent").unwrap();
//! #     assert_eq!(value, "my very cool app");
//! #     Ok::<_, std::convert::Infallible>(Response::empty())
//! # });
//! #
//! let mut svc = ServiceBuilder::new()
//!     .layer(
//!         // Layer that sets `User-Agent: my very cool app` on requests.
//!         //
//!         // `if_not_present` will only insert the header if it does not already
//!         // have a value.
//!         SetRequestHeaderLayer::if_not_present(
//!             "User-Agent".into(),
//!             "my very cool app".to_owned(),
//!         )
//!     )
//!     .service(client);
//!
//! let request = Request::empty();
//!
//! let response = svc.ready().await?.call(request).await?;
//! #
//! # Ok(())
//! # }
//! ```
//!
//! Setting a header based on a value determined dynamically from the request:
//!
//! ```
//! use anemo::{Request, Response};
//! use anemo_tower::set_header::SetRequestHeaderLayer;
//! use tower::{Service, ServiceExt, ServiceBuilder};
//! use bytes::Bytes;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let http_client = tower::service_fn(|req: Request<Bytes>| async move {
//! #     let value = req.headers().get("Date").unwrap();
//! #     assert_eq!(value, "now");
//! #     Ok::<_, std::convert::Infallible>(Response::empty())
//! # });
//! fn date_header_value() -> String {
//!     // ...
//!     # "now".to_owned()
//! }
//!
//! let mut svc = ServiceBuilder::new()
//!     .layer(
//!         // Layer that sets `Date` to the current date and time.
//!         //
//!         // `overriding` will insert the header and override any previous values it
//!         // may have.
//!         SetRequestHeaderLayer::overriding(
//!             "Date".into(),
//!             |request: &Request<Bytes>| {
//!                 Some(date_header_value())
//!             }
//!         )
//!     )
//!     .service(http_client);
//!
//! let request = Request::empty();
//!
//! let response = svc.ready().await?.call(request).await?;
//! #
//! # Ok(())
//! # }
//! ```

use super::{HeaderName, InsertHeaderMode, MakeHeaderValue};
use anemo::{Request, Response};
use std::{
    fmt,
    task::{Context, Poll},
};
use tower::Layer;
use tower::Service;

/// Layer that applies [`SetRequestHeader`] which adds a request header.
///
/// See [`SetRequestHeader`] for more details.
pub struct SetRequestHeaderLayer<M> {
    header_name: HeaderName,
    make: M,
    mode: InsertHeaderMode,
}

impl<M> fmt::Debug for SetRequestHeaderLayer<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetRequestHeaderLayer")
            .field("header_name", &self.header_name)
            .field("mode", &self.mode)
            .field("make", &std::any::type_name::<M>())
            .finish()
    }
}

impl<M> SetRequestHeaderLayer<M> {
    /// Create a new [`SetRequestHeaderLayer`].
    ///
    /// If a previous value exists for the same header, it is removed and replaced with the new
    /// header value.
    pub fn overriding(header_name: HeaderName, make: M) -> Self {
        Self::new(header_name, make, InsertHeaderMode::Override)
    }

    /// Create a new [`SetRequestHeaderLayer`].
    ///
    /// If a previous value exists for the header, the new value is not inserted.
    pub fn if_not_present(header_name: HeaderName, make: M) -> Self {
        Self::new(header_name, make, InsertHeaderMode::IfNotPresent)
    }

    fn new(header_name: HeaderName, make: M, mode: InsertHeaderMode) -> Self {
        Self {
            make,
            header_name,
            mode,
        }
    }
}

impl<S, M> Layer<S> for SetRequestHeaderLayer<M>
where
    M: Clone,
{
    type Service = SetRequestHeader<S, M>;

    fn layer(&self, inner: S) -> Self::Service {
        SetRequestHeader {
            inner,
            header_name: self.header_name.clone(),
            make: self.make.clone(),
            mode: self.mode,
        }
    }
}

impl<M> Clone for SetRequestHeaderLayer<M>
where
    M: Clone,
{
    fn clone(&self) -> Self {
        Self {
            make: self.make.clone(),
            header_name: self.header_name.clone(),
            mode: self.mode,
        }
    }
}

/// Middleware that sets a header on the request.
#[derive(Clone)]
pub struct SetRequestHeader<S, M> {
    inner: S,
    header_name: HeaderName,
    make: M,
    mode: InsertHeaderMode,
}

impl<S, M> SetRequestHeader<S, M> {
    /// Create a new [`SetRequestHeader`].
    ///
    /// If a previous value exists for the same header, it is removed and replaced with the new
    /// header value.
    pub fn overriding(inner: S, header_name: HeaderName, make: M) -> Self {
        Self::new(inner, header_name, make, InsertHeaderMode::Override)
    }

    /// Create a new [`SetRequestHeader`].
    ///
    /// If a previous value exists for the header, the new value is not inserted.
    pub fn if_not_present(inner: S, header_name: HeaderName, make: M) -> Self {
        Self::new(inner, header_name, make, InsertHeaderMode::IfNotPresent)
    }

    fn new(inner: S, header_name: HeaderName, make: M, mode: InsertHeaderMode) -> Self {
        Self {
            inner,
            header_name,
            make,
            mode,
        }
    }

    /// Gets a reference to the underlying service.
    pub fn inner(&self) -> &S {
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
}

impl<S, M> fmt::Debug for SetRequestHeader<S, M>
where
    S: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetRequestHeader")
            .field("inner", &self.inner)
            .field("header_name", &self.header_name)
            .field("mode", &self.mode)
            .field("make", &std::any::type_name::<M>())
            .finish()
    }
}

impl<ReqBody, ResBody, S, M> Service<Request<ReqBody>> for SetRequestHeader<S, M>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    M: MakeHeaderValue<Request<ReqBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        self.mode.apply(&self.header_name, &mut req, &mut self.make);
        self.inner.call(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::convert::Infallible;
    use tower::{service_fn, ServiceExt};

    const CONTENT_TYPE: &str = "content-type";

    #[tokio::test]
    async fn test_override_mode() {
        let svc = SetRequestHeader::overriding(
            service_fn(|req: Request<Bytes>| {
                let value = req.headers().get(CONTENT_TYPE);
                assert_eq!(value.unwrap(), "text/html");

                async { Ok::<_, Infallible>(Response::empty()) }
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        let request = Request::empty().with_header(CONTENT_TYPE, "good-content");
        svc.oneshot(request).await.unwrap();
    }

    #[tokio::test]
    async fn test_skip_if_present_mode() {
        let svc = SetRequestHeader::if_not_present(
            service_fn(|req: Request<Bytes>| {
                let value = req.headers().get(CONTENT_TYPE).unwrap();
                assert_eq!(value, "good-content");

                async { Ok::<_, Infallible>(Response::empty()) }
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        let request = Request::empty().with_header(CONTENT_TYPE, "good-content");
        svc.oneshot(request).await.unwrap();
    }

    #[tokio::test]
    async fn test_skip_if_present_mode_when_not_present() {
        let svc = SetRequestHeader::if_not_present(
            service_fn(|req: Request<Bytes>| {
                let value = req.headers().get(CONTENT_TYPE).unwrap();
                assert_eq!(value, "text/html");

                async { Ok::<_, Infallible>(Response::empty()) }
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        svc.oneshot(Request::empty()).await.unwrap();
    }
}
