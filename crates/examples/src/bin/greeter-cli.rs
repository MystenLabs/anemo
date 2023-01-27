use examples::{HelloRequest, HelloResponse};
use futures::FutureExt;
use greeter::greeter_client::GreeterClient;
mod greeter {
    include!(concat!(env!("OUT_DIR"), "/example.helloworld.Greeter.rs"));
}

#[tokio::main]
async fn main() {
    let config = anemo_cli::Config::new().add_service(
        "Greeter".to_string(),
        anemo_cli::ServiceInfo::new().add_method(
            "SayHello".to_string(),
            Box::new(|peer, request_str| {
                async move {
                    let request: HelloRequest =
                        ron::from_str(request_str.as_str()).expect("request text parses");
                    let mut client = GreeterClient::new(peer);
                    match client.say_hello(request).await {
                        Ok(response) => format!(
                            "successful response:\n{}",
                            ron::to_string(response.body()).unwrap_or_else(|err| format!(
                                "error converting response to text format: {err:?}"
                            ))
                        ),
                        Err(status) => format!("error response:\n{status:?}"),
                    }
                }
                .boxed()
            }),
        ),
    );

    anemo_cli::main(config).await;
}
