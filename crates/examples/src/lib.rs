use anemo::{rpc::Status, Request, Response};
use serde::{Deserialize, Serialize};
use tracing::info;

pub use greeter::{
    greeter_client::GreeterClient,
    greeter_server::{Greeter, GreeterServer},
};
pub mod greeter {
    include!(concat!(env!("OUT_DIR"), "/example.helloworld.Greeter.rs"));
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HelloRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HelloResponse {
    pub message: String,
}

#[derive(Default)]
pub struct MyGreeter {}

#[anemo::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloResponse>, Status> {
        info!(
            "Got a request from {}",
            request.peer_id().unwrap().short_display(4)
        );

        let reply = HelloResponse {
            message: format!("Hello {}!", request.into_body().name),
        };

        Ok(Response::new(reply))

        // Example for a server handler using the `server_handler_return_raw_bytes` option.
        // let mut reply_bytes = bytes::BytesMut::new();
        // bincode::serialize_into(reply_bytes.as_mut().writer(), &reply)
        //     .map_err(|e| anemo::rpc::Status::from_error(e.into()))?;
        // Ok(Response::new(reply_bytes.freeze()))
    }
}
