use super::Extensions;
use crate::{
    types::{HeaderMap, Version},
    PeerId,
};

#[derive(Debug)]
#[non_exhaustive]
pub struct RequestHeader {
    pub route: String,

    /// The request's version
    pub version: Version,

    /// The request's headers
    pub headers: HeaderMap,

    /// The request's extensions
    pub extensions: Extensions,
}

impl Default for RequestHeader {
    fn default() -> Self {
        Self {
            route: "/".into(),
            version: Default::default(),
            headers: Default::default(),
            extensions: Default::default(),
        }
    }
}

impl RequestHeader {
    pub(crate) fn from_raw(raw_header: RawRequestHeader, version: Version) -> Self {
        Self {
            route: raw_header.route,
            version,
            headers: raw_header.headers,
            extensions: Default::default(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RawRequestHeader {
    pub route: String,

    pub headers: HeaderMap,
}

impl RawRequestHeader {
    pub fn from_header(header: RequestHeader) -> Self {
        Self {
            route: header.route,
            headers: header.headers,
        }
    }
}

#[derive(Debug)]
pub struct Request<T> {
    head: RequestHeader,
    body: T,
}

impl<T> Request<T> {
    pub fn new(body: T) -> Request<T> {
        Self::from_parts(RequestHeader::default(), body)
    }

    pub fn from_parts(parts: RequestHeader, body: T) -> Request<T> {
        Self { head: parts, body }
    }

    pub fn route(&self) -> &str {
        &self.head.route
    }

    pub fn route_mut(&mut self) -> &mut String {
        &mut self.head.route
    }

    pub fn with_route<R: Into<String>>(mut self, route: R) -> Self {
        self.head.route = route.into();
        self
    }

    pub fn version(&self) -> Version {
        self.head.version
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    pub fn with_header<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.headers_mut().insert(key.into(), value.into());
        self
    }

    pub fn extensions(&self) -> &Extensions {
        &self.head.extensions
    }

    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.head.extensions
    }

    pub fn with_extension<E: Send + Sync + 'static>(mut self, extension: E) -> Self {
        self.extensions_mut().insert(extension);
        self
    }

    // Returns the PeerId of the peer who created this Request
    pub fn peer_id(&self) -> Option<&PeerId> {
        self.extensions().get::<PeerId>()
    }

    pub fn body(&self) -> &T {
        &self.body
    }

    pub fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    pub fn into_body(self) -> T {
        self.body
    }

    pub fn inner(&self) -> &T {
        &self.body
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.body
    }

    pub fn into_inner(self) -> T {
        self.body
    }

    pub fn into_parts(self) -> (RequestHeader, T) {
        (self.head, self.body)
    }

    pub fn map<F, U>(self, f: F) -> Request<U>
    where
        F: FnOnce(T) -> U,
    {
        Request {
            body: f(self.body),
            head: self.head,
        }
    }

    /// Set the max duration the request is allowed to take.
    ///
    /// The duration will be formatted as nanoseconds as an [`u64`] in the
    /// [`TIMEOUT`](super::header::TIMEOUT) header.
    ///
    /// Example:
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use anemo::Request;
    ///
    /// let mut request = Request::new(());
    ///
    /// request.set_timeout(Duration::from_secs(30));
    ///
    /// let value = request.headers().get("timeout").unwrap();
    ///
    /// assert_eq!(
    ///     value,
    ///     &Duration::from_secs(30).as_nanos().to_string(),
    /// );
    /// ```
    pub fn set_timeout(&mut self, timeout: std::time::Duration) {
        let timeout = crate::middleware::timeout::duration_to_timeout(timeout);
        self.headers_mut()
            .insert(super::header::TIMEOUT.into(), timeout);
    }

    /// Set the max duration the request is allowed to take.
    ///
    /// The duration will be formatted as nanoseconds as an [`u64`] in the
    /// [`TIMEOUT`](super::header::TIMEOUT) header.
    ///
    /// Example:
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use anemo::Request;
    ///
    /// let request = Request::new(()).with_timeout(Duration::from_secs(30));
    ///
    /// let value = request.headers().get("timeout").unwrap();
    ///
    /// assert_eq!(
    ///     value,
    ///     &Duration::from_secs(30).as_nanos().to_string(),
    /// );
    /// ```
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.set_timeout(timeout);
        self
    }

    /// Returns the previously-set timeout on this Request, or None if
    /// not set or invalid.
    ///
    /// Example:
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use anemo::Request;
    ///
    /// let request = Request::new(()).with_timeout(Duration::from_secs(30));
    /// let timeout = request.timeout().unwrap();
    ///
    /// assert_eq!(
    ///     timeout,
    ///     Duration::from_secs(30),
    /// );
    /// ```
    pub fn timeout(&self) -> Option<std::time::Duration> {
        crate::middleware::timeout::try_parse_timeout(self.headers())
            .ok()
            .flatten()
    }
}

impl Request<bytes::Bytes> {
    pub fn empty() -> Self {
        Self::new(bytes::Bytes::new())
    }
}

pub trait IntoRequest<T> {
    /// Wrap the input message `T` in a `Request`
    fn into_request(self) -> Request<T>;
}

impl<T> IntoRequest<T> for T {
    fn into_request(self) -> Request<Self> {
        Request::new(self)
    }
}

impl<T> IntoRequest<T> for Request<T> {
    fn into_request(self) -> Request<T> {
        self
    }
}
