use super::Extensions;
use crate::{
    types::{HeaderMap, Version},
    PeerId, Result,
};

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ResponseHeader {
    pub status: StatusCode,

    /// The response's version
    pub version: Version,

    pub headers: HeaderMap,

    /// The request's extensions
    pub extensions: Extensions,
}

impl ResponseHeader {
    pub(crate) fn from_raw(raw_header: RawResponseHeader, version: Version) -> Result<Self> {
        Ok(Self {
            status: StatusCode::new(raw_header.status)?,
            version,
            headers: raw_header.headers,
            extensions: Default::default(),
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RawResponseHeader {
    pub status: u16,

    pub headers: HeaderMap,
}

impl RawResponseHeader {
    pub fn from_header(header: ResponseHeader) -> (Self, Extensions) {
        (
            Self {
                status: header.status.to_u16(),
                headers: header.headers,
            },
            header.extensions,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
#[non_exhaustive]
pub enum StatusCode {
    Success = 200,
    BadRequest = 400,
    NotFound = 404,
    RequestTimeout = 408,
    TooManyRequests = 429,
    InternalServerError = 500,
    VersionNotSupported = 505,
    Unknown = 520,
}

impl StatusCode {
    pub fn new(code: u16) -> Result<Self, InvalidStatusCodeError> {
        use StatusCode::*;

        let status = match code {
            200 => Success,
            400 => BadRequest,
            404 => NotFound,
            408 => RequestTimeout,
            429 => TooManyRequests,
            500 => InternalServerError,
            505 => VersionNotSupported,
            520 => Unknown,
            _ => return Err(InvalidStatusCodeError(code)),
        };

        Ok(status)
    }

    #[inline]
    pub fn to_u16(self) -> u16 {
        self as u16
    }

    /// Check if status is within 200-299.
    #[inline]
    pub fn is_success(self) -> bool {
        200 <= self.to_u16() && self.to_u16() <= 299
    }

    /// Check if status is within 400-499.
    #[inline]
    pub fn is_client_error(self) -> bool {
        400 <= self.to_u16() && self.to_u16() <= 499
    }

    /// Check if status is within 500-599.
    #[inline]
    pub fn is_server_error(self) -> bool {
        500 <= self.to_u16() && self.to_u16() <= 599
    }
}

impl Default for StatusCode {
    fn default() -> Self {
        Self::Success
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use StatusCode::*;

        write!(f, "{} ", self.to_u16())?;
        match self {
            Success => f.write_str("Success"),
            BadRequest => f.write_str("Bad Request"),
            NotFound => f.write_str("Not Found"),
            TooManyRequests => f.write_str("Too Many Requests"),
            RequestTimeout => f.write_str("Request Timeout"),
            InternalServerError => f.write_str("Internal Server Error"),
            VersionNotSupported => f.write_str("Version Not Supported"),
            Unknown => f.write_str("Unknown"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvalidStatusCodeError(u16);

impl std::fmt::Display for InvalidStatusCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid StatusCode {}", self.0)
    }
}

impl std::error::Error for InvalidStatusCodeError {}

#[derive(Debug)]
pub struct Response<T> {
    head: ResponseHeader,
    body: T,
}

impl<T> Response<T> {
    pub fn new(body: T) -> Response<T> {
        Self::from_parts(ResponseHeader::default(), body)
    }

    pub fn from_parts(parts: ResponseHeader, body: T) -> Response<T> {
        Self { head: parts, body }
    }

    pub fn status(&self) -> StatusCode {
        self.head.status
    }

    pub fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.head.status
    }

    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.head.status = status;
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

    // Returns the PeerId of the peer who created this Response
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

    pub fn into_parts(self) -> (ResponseHeader, T) {
        (self.head, self.body)
    }

    pub fn map<F, U>(self, f: F) -> Response<U>
    where
        F: FnOnce(T) -> U,
    {
        Response {
            body: f(self.body),
            head: self.head,
        }
    }
}

impl Response<bytes::Bytes> {
    pub fn empty() -> Self {
        Self::new(bytes::Bytes::new())
    }
}

pub trait IntoResponse {
    /// Create a response.
    fn into_response(self) -> Response<bytes::Bytes>;
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response<bytes::Bytes> {
        let mut response = ().into_response();
        *response.status_mut() = self;
        response
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response<bytes::Bytes> {
        Response::new(bytes::Bytes::new())
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn all_status_code_variants_are_returned_from_new() {
        use super::{StatusCode, StatusCode::*};

        macro_rules! ensure_mapping {
            ($($variant:path),+ $(,)?) => {
                // assert that the given variants round-trip properly
                $(assert_eq!(StatusCode::new($variant.to_u16()), Ok($variant));)+

                // this generated fn will never be called but will produce a
                // non-exhaustive pattern error if you've missed a variant
                #[allow(dead_code)]
                fn check_all_covered(code: StatusCode) {
                    match code {
                        $($variant => {})+
                    };
                }
            }
        }

        ensure_mapping![
            Success,
            BadRequest,
            NotFound,
            TooManyRequests,
            RequestTimeout,
            InternalServerError,
            VersionNotSupported,
            Unknown,
        ];
    }
}
