use examples::{HelloRequest, HelloResponse};
use greeter::greeter_client::GreeterClient;
mod greeter {
    include!(concat!(env!("OUT_DIR"), "/example.helloworld.Greeter.rs"));
}

#[tokio::main]
async fn main() {
    let config = anemo_cli::Config::new().add_service(
        "Greeter",
        anemo_cli::ServiceInfo::new().add_method(
            "SayHello",
            anemo_cli::ron_method!(GreeterClient, say_hello, HelloRequest),
        ),
    );

    anemo_cli::main(config).await;
}
