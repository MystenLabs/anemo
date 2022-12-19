//! Middleware that adds high level [tracing] to a [`Service`].
//!
//! # Example
//!
//! Adding tracing to your service can be as simple as:
//!
//! ```rust
//! use anemo::{Request, Response};
//! use bytes::Bytes;
//! use tower::{ServiceBuilder, ServiceExt, Service};
//! use anemo_tower::trace::TraceLayer;
//! use std::convert::Infallible;
//!
//! async fn handle(request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
//!     Ok(Response::new(Bytes::from("foo")))
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Setup tracing
//! tracing_subscriber::fmt::init();
//!
//! let mut service = ServiceBuilder::new()
//!     .layer(TraceLayer::new_for_server_errors())
//!     .service_fn(handle);
//!
//! let request = Request::new(Bytes::from("foo"));
//!
//! let response = service
//!     .ready()
//!     .await?
//!     .call(request)
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! If you run this application with `RUST_LOG=anemo_tower=trace cargo run` you should see logs like:
//!
//! ```text
//! 2022-09-28T17:47:57.262533Z DEBUG request{route=/ version=V1}: anemo_tower::trace::on_request: started processing request
//! 2022-09-28T17:47:57.262562Z DEBUG request{route=/ version=V1}: anemo_tower::trace::on_response: finished processing request latency=0 ms status=200
//! ```
//!
//! # Customization
//!
//! [`Trace`] comes with good defaults but also supports customizing many aspects of the output.
//!
//! The default behaviour supports some customization:
//!
//! ```rust
//! use anemo::{Request, Response};
//! use bytes::Bytes;
//! use tower::ServiceBuilder;
//! use tracing::Level;
//! use anemo_tower::{
//!     LatencyUnit,
//!     trace::{TraceLayer, DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse},
//! };
//! use std::time::Duration;
//! # use tower::{ServiceExt, Service};
//! # use std::convert::Infallible;
//!
//! # async fn handle(request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
//! #     Ok(Response::new(Bytes::from("foo")))
//! # }
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tracing_subscriber::fmt::init();
//! #
//! let service = ServiceBuilder::new()
//!     .layer(
//!         TraceLayer::new_for_server_errors()
//!             .make_span_with(
//!                 DefaultMakeSpan::new().include_headers(true)
//!             )
//!             .on_request(
//!                 DefaultOnRequest::new().level(Level::INFO)
//!             )
//!             .on_response(
//!                 DefaultOnResponse::new()
//!                     .level(Level::INFO)
//!                     .latency_unit(LatencyUnit::Micros)
//!             )
//!             // on so on for `on_failure`
//!     )
//!     .service_fn(handle);
//! # let mut service = service;
//! # let response = service
//! #     .ready()
//! #     .await?
//! #     .call(Request::new(Bytes::from("foo")))
//! #     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! However for maximum control you can provide callbacks:
//!
//! ```rust
//! use anemo::{Request, Response};
//! use bytes::Bytes;
//! use tower::ServiceBuilder;
//! use anemo_tower::trace::TraceLayer;
//! use anemo_tower::classify::StatusInRangeFailureClass;
//! use std::time::Duration;
//! use tracing::Span;
//! # use tower::{ServiceExt, Service};
//! # use std::convert::Infallible;
//!
//! # async fn handle(request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
//! #     Ok(Response::new(Bytes::from("foo")))
//! # }
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tracing_subscriber::fmt::init();
//! #
//! let service = ServiceBuilder::new()
//!     .layer(
//!         TraceLayer::new_for_server_errors()
//!             .make_span_with(|request: &Request<Bytes>| {
//!                 tracing::debug_span!("anemo-request")
//!             })
//!             .on_request(|request: &Request<Bytes>, _span: &Span| {
//!                 tracing::debug!("started {}", request.route())
//!             })
//!             .on_response(|response: &Response<Bytes>, latency: Duration, _span: &Span| {
//!                 tracing::debug!("response generated in {:?}", latency)
//!             })
//!             .on_failure(|class: StatusInRangeFailureClass, latency: Duration, _span: &Span| {
//!                 tracing::debug!("something went wrong")
//!             })
//!     )
//!     .service_fn(handle);
//! # let mut service = service;
//! # let response = service
//! #     .ready()
//! #     .await?
//! #     .call(Request::new(Bytes::from("foo")))
//! #     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Disabling something
//!
//! Setting the behaviour to `()` will be disable that particular step:
//!
//! ```rust
//! use tower::ServiceBuilder;
//! use anemo_tower::trace::TraceLayer;
//! use anemo_tower::classify::StatusInRangeFailureClass;
//! use std::time::Duration;
//! use tracing::Span;
//! # use tower::{ServiceExt, Service};
//! # use bytes::Bytes;
//! # use anemo::{Response, Request};
//! # use std::convert::Infallible;
//!
//! # async fn handle(request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
//! #     Ok(Response::new(Bytes::from("foo")))
//! # }
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tracing_subscriber::fmt::init();
//! #
//! let service = ServiceBuilder::new()
//!     .layer(
//!         // This configuration will only emit events on failures
//!         TraceLayer::new_for_server_errors()
//!             .on_request(())
//!             .on_response(())
//!             .on_failure(|class: StatusInRangeFailureClass, latency: Duration, _span: &Span| {
//!                 tracing::debug!("something went wrong")
//!             })
//!     )
//!     .service_fn(handle);
//! # let mut service = service;
//! # let response = service
//! #     .ready()
//! #     .await?
//! #     .call(Request::new(Bytes::from("foo")))
//! #     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # When the callbacks are called
//!
//! ### `on_request`
//!
//! The `on_request` callback is called when the request arrives at the
//! middleware in [`Service::call`] just prior to passing the request to the
//! inner service.
//!
//! ### `on_response`
//!
//! The `on_response` callback is called when the inner service's response
//! future completes with `Ok(response)` regardless if the response is
//! classified as a success or a failure.
//!
//! For example if you're using [`StatusInRangeAsFailures::new_for_server_errors`] as your
//! classifier and the inner service responds with `500 Internal Server Error` then the
//! `on_response` callback is still called. `on_failure` would _also_ be called in this case since
//! the response was classified as a failure.
//!
//! ### `on_failure`
//!
//! The `on_failure` callback is called when:
//!
//! - The inner [`Service`]'s response future resolves to an error.
//! - A response is classified as a failure.
//!
//! # Recording fields on the span
//!
//! All callbacks receive a reference to the [tracing] [`Span`], corresponding to this request,
//! produced by the closure passed to [`TraceLayer::make_span_with`]. It can be used to [record
//! field values][record] that weren't known when the span was created.
//!
//! ```rust
//! use anemo::{Request, Response};
//! use bytes::Bytes;
//! use tower::ServiceBuilder;
//! use anemo_tower::trace::TraceLayer;
//! use tracing::Span;
//! use std::time::Duration;
//! # use std::convert::Infallible;
//!
//! # async fn handle(request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
//! #     Ok(Response::new(Bytes::from("foo")))
//! # }
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # tracing_subscriber::fmt::init();
//! #
//! let service = ServiceBuilder::new()
//!     .layer(
//!         TraceLayer::new_for_server_errors()
//!             .make_span_with(|request: &Request<Bytes>| {
//!                 tracing::debug_span!(
//!                     "anemo-request",
//!                     status_code = tracing::field::Empty,
//!                 )
//!             })
//!             .on_response(|response: &Response<Bytes>, _latency: Duration, span: &Span| {
//!                 span.record("status_code", &tracing::field::display(response.status().to_u16()));
//!
//!                 tracing::debug!("response generated")
//!             })
//!     )
//!     .service_fn(handle);
//! # Ok(())
//! # }
//! ```
//!
//!
//! [tracing]: https://crates.io/crates/tracing
//! [`Service`]: tower::Service
//! [`Service::call`]: tower::Service::call
//! [record]: https://docs.rs/tracing/latest/tracing/span/struct.Span.html#method.record
//! [`StatusInRangeAsFailures::new_for_server_errors`]: crate::classify::StatusInRangeAsFailures::new_for_server_errors
//! [`TraceLayer::make_span_with`]: crate::trace::TraceLayer::make_span_with
//! [`Span`]: tracing::Span

use tracing::Level;

pub use self::{
    future::ResponseFuture,
    layer::TraceLayer,
    make_span::{DefaultMakeSpan, MakeSpan},
    on_failure::{DefaultOnFailure, OnFailure},
    on_request::{DefaultOnRequest, OnRequest},
    on_response::{DefaultOnResponse, OnResponse},
    service::Trace,
};

mod future;
mod layer;
mod make_span;
mod on_failure;
mod on_request;
mod on_response;
mod service;

const DEFAULT_MESSAGE_LEVEL: Level = Level::DEBUG;
const DEFAULT_FAILURE_LEVEL: Level = Level::WARN;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::StatusInRangeFailureClass;
    use anemo::{Request, Response};
    use bytes::Bytes;
    use once_cell::sync::Lazy;
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::Duration,
    };
    use tower::{BoxError, Service, ServiceBuilder, ServiceExt};
    use tracing::Span;

    #[tokio::test]
    async fn unary_request() {
        static ON_REQUEST_COUNT: Lazy<AtomicU32> = Lazy::new(|| AtomicU32::new(0));
        static ON_RESPONSE_COUNT: Lazy<AtomicU32> = Lazy::new(|| AtomicU32::new(0));
        static ON_FAILURE: Lazy<AtomicU32> = Lazy::new(|| AtomicU32::new(0));

        let trace_layer = TraceLayer::new_for_server_errors()
            .make_span_with(|_req: &Request<Bytes>| {
                tracing::info_span!("test-span", foo = tracing::field::Empty)
            })
            .on_request(|_req: &Request<Bytes>, span: &Span| {
                span.record("foo", 42);
                ON_REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
            })
            .on_response(|_res: &Response<Bytes>, _latency: Duration, _span: &Span| {
                ON_RESPONSE_COUNT.fetch_add(1, Ordering::SeqCst);
            })
            .on_failure(
                |_class: StatusInRangeFailureClass, _latency: Duration, _span: &Span| {
                    ON_FAILURE.fetch_add(1, Ordering::SeqCst);
                },
            );

        let mut svc = ServiceBuilder::new().layer(trace_layer).service_fn(echo);

        let _res = svc
            .ready()
            .await
            .unwrap()
            .call(Request::new(Bytes::from("foobar")))
            .await
            .unwrap();

        assert_eq!(1, ON_REQUEST_COUNT.load(Ordering::SeqCst), "request");
        assert_eq!(1, ON_RESPONSE_COUNT.load(Ordering::SeqCst), "response");
        assert_eq!(0, ON_FAILURE.load(Ordering::SeqCst), "failure");
    }

    async fn echo(req: Request<Bytes>) -> Result<Response<Bytes>, BoxError> {
        Ok(Response::new(req.into_body()))
    }
}
