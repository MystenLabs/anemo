use crate::{
    error::BoxError,
    types::{
        response::{IntoResponse, StatusCode},
        HeaderMap,
    },
    Response,
};

pub mod codec;

#[derive(Debug)]
pub struct Status {
    status: StatusCode,
    headers: HeaderMap,
    message: Option<String>,

    /// Optional underlying error.
    source: Option<BoxError>,
}

impl Status {
    /// Create a new `Status` with the associated code and message.
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            message: None,
            headers: HeaderMap::default(),
            source: None,
        }
    }

    pub fn new_with_message<M: Into<String>>(status: StatusCode, message: M) -> Self {
        let mut status = Self::new(status);
        status.message = Some(message.into());
        status
    }

    pub fn unknown<M: Into<String>>(message: M) -> Self {
        Self::new_with_message(StatusCode::Unknown, message)
    }

    pub fn internal<M: Into<String>>(message: M) -> Self {
        Self::new_with_message(StatusCode::InternalServerError, message)
    }

    pub fn from_error(error: BoxError) -> Status {
        let mut status = Self::new(StatusCode::Unknown);
        status.message = Some(format!("unknown error: {error}"));
        status.source = Some(error);
        status
    }
}

impl IntoResponse for Status {
    fn into_response(self) -> Response<bytes::Bytes> {
        let mut response = self.status.into_response();

        response.headers_mut().extend(self.headers);

        if let Some(message) = self.message {
            response
                .headers_mut()
                .insert(crate::types::header::STATUS_MESSAGE.to_owned(), message);
        }

        response
    }
}

pub mod client {
    use super::{
        codec::{Codec, Decoder, Encoder},
        Status,
    };
    use crate::{error::BoxError, Request, Response};
    use bytes::{Bytes, BytesMut};
    use tower::Service;

    #[derive(Debug, Clone)]
    pub struct Rpc<T> {
        inner: T,
    }

    impl<T> Rpc<T> {
        pub fn new(inner: T) -> Self {
            Self { inner }
        }

        /// Gets a reference to the underlying service.
        pub fn inner(&self) -> &T {
            &self.inner
        }

        /// Gets a mutable reference to the underlying service.
        pub fn inner_mut(&mut self) -> &mut T {
            &mut self.inner
        }

        /// Consumes `self`, returning the underlying service.
        pub fn into_inner(self) -> T {
            self.inner
        }

        pub async fn ready(&mut self) -> Result<(), T::Error>
        where
            T: Service<Request<Bytes>>,
        {
            futures::future::poll_fn(|cx| self.inner.poll_ready(cx)).await
        }

        /// Send a single unary RPC request.
        pub async fn unary<M1, M2, C>(
            &mut self,
            request: Request<M1>,
            mut codec: C,
        ) -> Result<Response<M2>, Status>
        where
            T: Service<Request<Bytes>, Response = Response<Bytes>>,
            T::Error: Into<BoxError>,
            C: Codec<Encode = M1, Decode = M2>,
            M1: Send + Sync + 'static,
            M2: Send + Sync + 'static,
        {
            let request = {
                let (mut parts, body) = request.into_parts();

                // Set the content type
                parts.headers.insert(
                    crate::types::header::CONTENT_TYPE.to_owned(),
                    C::FORMAT_NAME.to_owned(),
                );

                let mut bytes = BytesMut::new();
                let mut encoder = codec.encoder();
                encoder
                    .encode(body, &mut bytes)
                    .map_err(Into::into)
                    .map_err(Status::from_error)?;

                Request::from_parts(parts, bytes.freeze())
            };

            let response = self
                .inner
                .call(request)
                .await
                .map_err(Into::into)
                .map_err(Status::from_error)?;

            let status_code = response.status();

            if !status_code.is_success() {
                return Err(Status::new(status_code));
            }

            let response = {
                let (parts, body) = response.into_parts();

                let mut decoder = codec.decoder();
                let message = decoder
                    .decode(body)
                    .map_err(Into::into)
                    .map_err(Status::from_error)?;

                Response::from_parts(parts, message)
            };

            Ok(response)
        }
    }
}

pub mod server {
    use bytes::{Bytes, BytesMut};
    use tower::Service;

    use crate::{rpc::codec::Decoder, types::response::IntoResponse, Request, Response};

    use super::{
        codec::{Codec, Encoder},
        Status,
    };
    use std::future::Future;

    pub struct Rpc<T> {
        codec: T,
    }

    impl<T> Rpc<T>
    where
        T: Codec,
    {
        pub fn new(codec: T) -> Self {
            Self { codec }
        }

        /// Handle a single unary RPC request.
        pub async fn unary<S>(&mut self, mut service: S, request: Request<Bytes>) -> Response<Bytes>
        where
            S: UnaryService<T::Decode, Response = T::Encode>,
        {
            let request = match self.map_request(request).await {
                Ok(r) => r,
                Err(status) => {
                    return self.map_response(Err(status));
                }
            };

            let response = service.call(request).await;

            self.map_response(response)
        }

        async fn map_request(
            &mut self,
            request: Request<Bytes>,
        ) -> Result<Request<T::Decode>, Status> {
            let (parts, body) = request.into_parts();

            let mut decoder = self.codec.decoder();
            let message = decoder
                .decode(body)
                .map_err(Into::into)
                .map_err(Status::from_error)?;

            let req = Request::from_parts(parts, message);

            Ok(req)
        }

        fn map_response(
            &mut self,
            response: Result<crate::Response<T::Encode>, Status>,
        ) -> Response<Bytes> {
            let response = match response {
                Ok(r) => r,
                Err(status) => return status.into_response(),
            };

            let (mut parts, body) = response.into_parts();

            // Set the content type
            parts.headers.insert(
                crate::types::header::CONTENT_TYPE.to_owned(),
                T::FORMAT_NAME.to_owned(),
            );

            let mut bytes = BytesMut::new();
            let mut encoder = self.codec.encoder();

            if let Err(status) = encoder
                .encode(body, &mut bytes)
                .map_err(Into::into)
                .map_err(|err| Status::internal(format!("Error encoding: {err}")))
            {
                return status.into_response();
            }

            Response::from_parts(parts, bytes.freeze())
        }
    }

    /// A specialization of tower_service::Service.
    ///
    /// Existing tower_service::Service implementations with the correct form will
    /// automatically implement `UnaryService`.
    pub trait UnaryService<R> {
        /// Response message type
        type Response;

        /// Response future
        type Future: Future<Output = Result<Response<Self::Response>, Status>>;

        /// Call the service
        fn call(&mut self, request: Request<R>) -> Self::Future;
    }

    impl<T, M1, M2> UnaryService<M1> for T
    where
        T: Service<Request<M1>, Response = Response<M2>, Error = Status>,
    {
        type Response = M2;
        type Future = T::Future;

        fn call(&mut self, request: Request<M1>) -> Self::Future {
            Service::call(self, request)
        }
    }
}

pub trait RpcService {
    const SERVICE_NAME: &'static str;
}
