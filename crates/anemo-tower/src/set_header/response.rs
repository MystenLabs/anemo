//! Set a header on the response.
//!
//! The header value to be set may be provided as a fixed value when the
//! middleware is constructed, or determined dynamically based on the response
//! by a closure. See the [`MakeHeaderValue`] trait for details.
//!
//! # Example
//!
//! Setting a header from a fixed value provided when the middleware is constructed:
//!
//! ```
//! use anemo::{Request, Response};
//! use anemo_tower::set_header::SetResponseHeaderLayer;
//! use tower::{Service, ServiceExt, ServiceBuilder};
//! use bytes::Bytes;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let render_html = tower::service_fn(|request: Request<Bytes>| async move {
//! #     Ok::<_, std::convert::Infallible>(Response::new(request.into_body()))
//! # });
//! #
//! let mut svc = ServiceBuilder::new()
//!     .layer(
//!         // Layer that sets `content-type: text/html` on responses.
//!         //
//!         // `if_not_present` will only insert the header if it does not already
//!         // have a value.
//!         SetResponseHeaderLayer::if_not_present(
//!             "content-type".into(),
//!             "text/html".to_owned(),
//!         )
//!     )
//!     .service(render_html);
//!
//! let request = Request::empty();
//!
//! let response = svc.ready().await?.call(request).await?;
//!
//! assert_eq!(response.headers()["content-type"], "text/html");
//! #
//! # Ok(())
//! # }
//! ```
//!
//! Setting a header based on a value determined dynamically from the response:
//!
//! ```
//! use anemo::{Request, Response};
//! use anemo_tower::set_header::SetResponseHeaderLayer;
//! use tower::{Service, ServiceExt, ServiceBuilder};
//! use bytes::Bytes;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let render_html = tower::service_fn(|request: Request<Bytes>| async move {
//! #     Ok::<_, std::convert::Infallible>(Response::new(Bytes::from("1234567890")))
//! # });
//! #
//! let mut svc = ServiceBuilder::new()
//!     .layer(
//!         // Layer that sets `content-length` if the body has a known size.
//!         //
//!         // `overriding` will insert the header and override any previous values it
//!         // may have.
//!         SetResponseHeaderLayer::overriding(
//!             "content-length".into(),
//!             |response: &Response<Bytes>| {
//!                 Some(response.body().len().to_string())
//!             }
//!         )
//!     )
//!     .service(render_html);
//!
//! let request = Request::empty();
//!
//! let response = svc.ready().await?.call(request).await?;
//!
//! assert_eq!(response.headers()["content-length"], "10");
//! #
//! # Ok(())
//! # }
//! ```

use super::{HeaderName, InsertHeaderMode, MakeHeaderValue};
use anemo::{Request, Response};
use pin_project_lite::pin_project;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::Layer;
use tower::Service;

/// Layer that applies [`SetResponseHeader`] which adds a response header.
///
/// See [`SetResponseHeader`] for more details.
pub struct SetResponseHeaderLayer<M> {
    header_name: HeaderName,
    make: M,
    mode: InsertHeaderMode,
}

impl<M> fmt::Debug for SetResponseHeaderLayer<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetResponseHeaderLayer")
            .field("header_name", &self.header_name)
            .field("mode", &self.mode)
            .field("make", &std::any::type_name::<M>())
            .finish()
    }
}

impl<M> SetResponseHeaderLayer<M> {
    /// Create a new [`SetResponseHeaderLayer`].
    ///
    /// If a previous value exists for the same header, it is removed and replaced with the new
    /// header value.
    pub fn overriding(header_name: HeaderName, make: M) -> Self {
        Self::new(header_name, make, InsertHeaderMode::Override)
    }

    /// Create a new [`SetResponseHeaderLayer`].
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

impl<S, M> Layer<S> for SetResponseHeaderLayer<M>
where
    M: Clone,
{
    type Service = SetResponseHeader<S, M>;

    fn layer(&self, inner: S) -> Self::Service {
        SetResponseHeader {
            inner,
            header_name: self.header_name.clone(),
            make: self.make.clone(),
            mode: self.mode,
        }
    }
}

impl<M> Clone for SetResponseHeaderLayer<M>
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

/// Middleware that sets a header on the response.
#[derive(Clone)]
pub struct SetResponseHeader<S, M> {
    inner: S,
    header_name: HeaderName,
    make: M,
    mode: InsertHeaderMode,
}

impl<S, M> SetResponseHeader<S, M> {
    /// Create a new [`SetResponseHeader`].
    ///
    /// If a previous value exists for the same header, it is removed and replaced with the new
    /// header value.
    pub fn overriding(inner: S, header_name: HeaderName, make: M) -> Self {
        Self::new(inner, header_name, make, InsertHeaderMode::Override)
    }

    /// Create a new [`SetResponseHeader`].
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

impl<S, M> fmt::Debug for SetResponseHeader<S, M>
where
    S: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetResponseHeader")
            .field("inner", &self.inner)
            .field("header_name", &self.header_name)
            .field("mode", &self.mode)
            .field("make", &std::any::type_name::<M>())
            .finish()
    }
}

impl<ReqBody, ResBody, S, M> Service<Request<ReqBody>> for SetResponseHeader<S, M>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    M: MakeHeaderValue<Response<ResBody>> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, M>;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        ResponseFuture {
            future: self.inner.call(req),
            header_name: self.header_name.clone(),
            make: self.make.clone(),
            mode: self.mode,
        }
    }
}

pin_project! {
    /// Response future for [`SetResponseHeader`].
    #[derive(Debug)]
    pub struct ResponseFuture<F, M> {
        #[pin]
        future: F,
        header_name: HeaderName,
        make: M,
        mode: InsertHeaderMode,
    }
}

impl<F, ResBody, E, M> Future for ResponseFuture<F, M>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
    M: MakeHeaderValue<Response<ResBody>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut res = futures::ready!(this.future.poll(cx)?);

        this.mode.apply(this.header_name, &mut res, &mut *this.make);

        Poll::Ready(Ok(res))
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
        let svc = SetResponseHeader::overriding(
            service_fn(|_req: Request<Bytes>| async {
                let res = Response::empty().with_header(CONTENT_TYPE, "good-content");
                Ok::<_, Infallible>(res)
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        let res = svc.oneshot(Request::empty()).await.unwrap();

        let value = res.headers().get(CONTENT_TYPE);
        assert_eq!(value.unwrap(), "text/html");
    }

    #[tokio::test]
    async fn test_skip_if_present_mode() {
        let svc = SetResponseHeader::if_not_present(
            service_fn(|_req: Request<Bytes>| async {
                let res = Response::empty().with_header(CONTENT_TYPE, "good-content");
                Ok::<_, Infallible>(res)
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        let res = svc.oneshot(Request::empty()).await.unwrap();

        let value = res.headers().get(CONTENT_TYPE).unwrap();
        assert_eq!(value, "good-content");
    }

    #[tokio::test]
    async fn test_skip_if_present_mode_when_not_present() {
        let svc = SetResponseHeader::if_not_present(
            service_fn(|_req: Request<Bytes>| async {
                let res = Response::empty();
                Ok::<_, Infallible>(res)
            }),
            CONTENT_TYPE.into(),
            "text/html".to_owned(),
        );

        let res = svc.oneshot(Request::empty()).await.unwrap();

        let value = res.headers().get(CONTENT_TYPE).unwrap();
        assert_eq!(value, "text/html");
    }
}
