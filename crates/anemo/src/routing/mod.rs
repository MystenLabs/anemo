use crate::{Request, Response};
use bytes::Bytes;
use std::{collections::HashMap, convert::Infallible, fmt, sync::Arc};
use tower::{
    util::{BoxCloneService, Oneshot},
    Service,
};

mod not_found;
mod route;

use route::Route;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RouteId(u32);

impl RouteId {
    fn next() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        // `AtomicU64` isn't supported on all platforms
        static ID: AtomicU32 = AtomicU32::new(0);
        let id = ID.fetch_add(1, Ordering::Relaxed);
        if id == u32::MAX {
            panic!("Over `u32::MAX` routes created. If you need this, please file an issue.");
        }
        Self(id)
    }
}

/// The router type for composing handlers and services.
#[derive(Clone)]
pub struct Router {
    routes: HashMap<RouteId, Route>,
    matcher: RouteMatcher,
    fallback: Route,
}

impl Router {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            routes: Default::default(),
            matcher: Default::default(),
            fallback: Route::new(not_found::NotFound),
        }
    }

    pub fn route<T>(mut self, path: &str, service: T) -> Self
    where
        T: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>
            + Clone
            + Send
            + 'static,
        T::Future: Send + 'static,
    {
        if path.is_empty() {
            panic!("Paths must start with a `/`. Use \"/\" for root routes");
        } else if !path.starts_with('/') {
            panic!("Paths must start with a `/`");
        }

        if <dyn std::any::Any>::downcast_ref::<Self>(&service).is_some() {
            panic!("Invalid route: `Router::route` cannot be used with `Router`s.")
        }

        let id = RouteId::next();

        let service =
            try_downcast::<Route, _>(service).unwrap_or_else(|service| Route::new(service));

        if let Err(err) = self.matcher.insert(path, id) {
            panic!("Invalid route: {err}");
        }

        self.routes.insert(id, service);

        self
    }

    pub fn add_rpc_service<S>(self, service: S) -> Self
    where
        S: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>
            + crate::rpc::RpcService
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let path = format!("/{}/*rest", S::SERVICE_NAME);
        self.route(&path, service)
    }

    /// Merge two routers into one.
    ///
    /// This is useful for breaking apps into smaller pieces and combining them
    /// into one.
    pub fn merge<R>(mut self, other: R) -> Self
    where
        R: Into<Router>,
    {
        let Router {
            routes,
            matcher,
            fallback,
        } = other.into();

        for (id, route) in routes {
            let path = matcher
                .route_id_to_path
                .get(&id)
                .expect("no path for route id. This is a bug in anemo. Please file an issue");
            self = self.route(path, route);
        }

        // TODO if we support custom fallback functions in the future we need to make sure to check
        // that we dont try to merge two routers that both have custom fallbacks
        let _fallback = fallback;

        self
    }

    /// Apply a [`tower::Layer`] to the router that will only run if the request matches
    /// a route.
    pub fn route_layer<L>(self, layer: L) -> Self
    where
        L: tower::Layer<Route>,
        L::Service: Service<Request<Bytes>, Response = Response<Bytes>, Error = Infallible>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<Request<Bytes>>>::Future: Send + 'static,
    {
        let Router {
            routes,
            matcher,
            fallback,
        } = self;

        let routes = routes
            .into_iter()
            .map(|(id, route)| {
                let route = Route::new(layer.layer(route));
                (id, route)
            })
            .collect();

        Router {
            routes,
            matcher,
            fallback,
        }
    }
}

impl Service<Request<Bytes>> for Router {
    type Response = Response<Bytes>;
    type Error = Infallible;
    type Future =
        Oneshot<BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible>, Request<Bytes>>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, req: Request<Bytes>) -> Self::Future {
        use matchit::MatchError;

        let path = req.route();

        match self.matcher.at(path) {
            Ok(match_) => {
                let route = self
                    .routes
                    .get(match_.value)
                    .expect("no route for id; this is a bug");
                route.oneshot_inner(req)
            }
            Err(MatchError::MissingTrailingSlash)
            | Err(MatchError::ExtraTrailingSlash)
            | Err(MatchError::NotFound) => self.fallback.oneshot_inner(req),
        }
    }
}

/// Wrapper around `matchit::Router` that supports merging two `Router`s.
#[derive(Clone, Default)]
struct RouteMatcher {
    inner: matchit::Router<RouteId>,
    route_id_to_path: HashMap<RouteId, Arc<str>>,
    path_to_route_id: HashMap<Arc<str>, RouteId>,
}

impl RouteMatcher {
    fn insert(
        &mut self,
        path: impl Into<String>,
        val: RouteId,
    ) -> Result<(), matchit::InsertError> {
        let path = path.into();

        self.inner.insert(&path, val)?;

        let shared_path: Arc<str> = path.into();
        self.route_id_to_path.insert(val, shared_path.clone());
        self.path_to_route_id.insert(shared_path, val);

        Ok(())
    }

    fn at<'n, 'p>(
        &'n self,
        path: &'p str,
    ) -> Result<matchit::Match<'n, 'p, &'n RouteId>, matchit::MatchError> {
        self.inner.at(path)
    }
}

impl fmt::Debug for RouteMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteMatcher")
            .field("paths", &self.route_id_to_path)
            .finish()
    }
}

pub(crate) fn try_downcast<T, K>(k: K) -> Result<T, K>
where
    T: 'static,
    K: Send + 'static,
{
    let mut k = Some(k);
    if let Some(k) = <dyn std::any::Any>::downcast_mut::<Option<T>>(&mut k) {
        Ok(k.take().unwrap())
    } else {
        Err(k.unwrap())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::types::response::{IntoResponse, StatusCode};
    use tower::{util::BoxCloneService, ServiceExt};
    use tracing::trace;

    #[test]
    fn router_add_simple_route() {
        let router = Router::new();
        router.route("/echo", echo_service());
    }

    #[test]
    #[should_panic(expected = "Paths must start with a `/`.")]
    fn router_empty_path() {
        let router = Router::new();
        router.route("", echo_service());
    }

    #[test]
    #[should_panic(expected = "Paths must start with a `/`")]
    fn router_path_does_not_start_with_slash() {
        let router = Router::new();
        router.route("", echo_service());
    }

    #[test]
    #[should_panic(expected = "Invalid route: `Router::route` cannot be used with `Router`s.")]
    fn router_cant_use_router_as_route_service() {
        let router = Router::new();
        router.route("/nested", Router::new());
    }

    #[tokio::test]
    async fn router_with_multiple_routes() {
        let hello_world = tower::service_fn(|_request| async {
            Ok(Response::new(Bytes::from_static(b"hello world!")))
        });

        let internal_server_error = tower::service_fn(|_request| async {
            Ok(StatusCode::InternalServerError.into_response())
        });

        let router = Router::new()
            .route("/echo", echo_service())
            .route("/hello-world", hello_world)
            .route("/internal/server/error", internal_server_error);

        // Not Found Route
        let request = Request::new(Bytes::new());
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NotFound);

        // Echo
        let msg = b"echo this text";
        let mut request = Request::new(Bytes::from_static(msg));
        *request.route_mut() = "/echo".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), msg.as_ref());

        // Hello World
        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello-world".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");

        // Internal server error
        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/internal/server/error".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::InternalServerError);
    }

    #[tokio::test]
    async fn route_with_wildcard() {
        let hello_world_msg = b"hello world!";
        let hello_world = tower::service_fn(|_request| async {
            Ok(Response::new(Bytes::from_static(hello_world_msg)))
        });

        let router = Router::new().route("/hello/*rest", hello_world);

        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello/".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");

        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "/hello/world".into();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");
    }

    #[tokio::test]
    async fn route_path_without_starting_slash() {
        let router = Router::new().route("/echo", echo_service());

        let mut request = Request::new(Bytes::new());
        *request.route_mut() = "echo".into();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NotFound);
    }

    fn echo_service() -> BoxCloneService<Request<Bytes>, Response<Bytes>, Infallible> {
        let handle = move |request: Request<Bytes>| async move {
            trace!("recieved: {}", request.body().escape_ascii());
            let response = Response::new(request.into_body());
            Result::<Response<Bytes>, Infallible>::Ok(response)
        };

        tower::service_fn(handle).boxed_clone()
    }

    #[tokio::test]
    async fn merge_router() {
        let hello_world = tower::service_fn(|_request| async {
            Ok(Response::new(Bytes::from_static(b"hello world!")))
        });

        let hello_world_router = Router::new().route("/hello-world", hello_world);
        let echo_router = Router::new().route("/echo", echo_service());
        let router = Router::new().merge(hello_world_router).merge(echo_router);

        // Echo
        let msg = b"echo this text";
        let request = Request::new(Bytes::from_static(msg)).with_route("/echo");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), msg.as_ref());

        // Hello World
        let request = Request::new(Bytes::new()).with_route("/hello-world");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
        assert_eq!(response.body(), "hello world!");
    }

    #[test]
    #[should_panic(
        expected = "Invalid route: insertion failed due to conflict with previously registered route"
    )]
    fn merge_router_with_conflicting_routes() {
        let echo_router_1 = Router::new().route("/echo", echo_service());
        let echo_router_2 = Router::new().route("/echo", echo_service());
        Router::new().merge(echo_router_1).merge(echo_router_2);
    }

    #[test]
    #[should_panic(
        expected = "Invalid route: insertion failed due to conflict with previously registered route"
    )]
    fn add_two_conflicting_routes() {
        Router::new()
            .route("/echo", echo_service())
            .route("/echo", echo_service());
    }

    #[test]
    fn test_try_downcast() {
        assert_eq!(super::try_downcast::<i32, _>(5_u32), Err(5_u32));
        assert_eq!(super::try_downcast::<i32, _>(5_i32), Ok(5_i32));
    }

    #[tokio::test]
    async fn middleware_applies_to_routes_above() {
        let pending =
            tower::service_fn(|_request| async { std::future::pending::<Result<_, _>>().await });
        let ready = tower::service_fn(|_request| async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Ok(Response::new(Bytes::from_static(b"ready!")))
        });
        let router = Router::new()
            .route("/one", pending)
            .route_layer(crate::middleware::timeout::inbound::TimeoutLayer::new(
                Some(std::time::Duration::new(0, 0)),
            ))
            .route("/two", ready);

        let request = Request::new(Bytes::new()).with_route("/one");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::RequestTimeout);

        let request = Request::new(Bytes::new()).with_route("/two");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
    }

    #[tokio::test]
    async fn middleware_applies_to_routes_after_merge() {
        let pending =
            tower::service_fn(|_request| async { std::future::pending::<Result<_, _>>().await });
        let ready = tower::service_fn(|_request| async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Ok(Response::new(Bytes::from_static(b"ready!")))
        });
        let pending = Router::new().route("/one", pending).route_layer(
            crate::middleware::timeout::inbound::TimeoutLayer::new(Some(std::time::Duration::new(
                0, 0,
            ))),
        );
        let ready = Router::new().route("/two", ready);
        let router = Router::new().merge(pending).merge(ready);

        let request = Request::new(Bytes::new()).with_route("/one");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::RequestTimeout);

        let request = Request::new(Bytes::new()).with_route("/two");
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::Success);
    }
}
