use anemo::codegen::BoxFuture;
use clap::{Parser, Subcommand};
use std::collections::HashMap;

pub mod call;
pub mod ping;
pub mod util;

pub struct Config {
    pub(crate) service_map: HashMap<String, ServiceInfo>,
}

impl Config {
    pub fn new() -> Self {
        Self {
            service_map: HashMap::new(),
        }
    }

    pub fn add_service(mut self, name: &str, info: ServiceInfo) -> Self {
        assert!(
            self.service_map.insert(name.to_string(), info).is_none(),
            "no duplicate services"
        );
        self
    }
}
impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

// Takes peer and text-formatted request message, outputs text-formatted response (or error).
pub type MethodFn = Box<dyn Fn(anemo::Peer, String) -> BoxFuture<'static, String>>;
pub struct ServiceInfo {
    pub(crate) method_map: HashMap<String, MethodFn>,
}

impl ServiceInfo {
    pub fn new() -> Self {
        Self {
            method_map: HashMap::new(),
        }
    }

    pub fn add_method(mut self, name: &str, method_fn: MethodFn) -> Self {
        assert!(
            self.method_map
                .insert(name.to_string(), method_fn)
                .is_none(),
            "no duplicate methods"
        );
        self
    }
}
impl Default for ServiceInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates a default MethodFn implementation using RON (Rusty Object Notation) format to
/// parse requests and display responses.
///
/// # Arguments
///
/// * `client_type` - The generated client struct to use, e.g. `MyServiceClient`
/// * `method_name` - The RPC method to call, e.g. `my_method`
/// * `request_type` - The type of the RPC request message, e.g. `MyMethodRequest`
#[macro_export]
macro_rules! ron_method {
    ($client_type: ty, $method_name: ident, $request_type: ty) => {
        Box::new(|peer, request_str| {
            use futures::FutureExt;
            async move {
                let request: $request_type =
                    ron::from_str(request_str.as_str()).expect("request text parses");
                let mut client = {
                    use $client_type as client_type;
                    client_type::new(peer)
                };
                match client.$method_name(request).await {
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
        })
    };
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    args: Args,
}

/// Network tools for interacting with Anemo servers
#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Connects to the indicated peer without doing anything.
    Ping {
        /// Address of the target to connect to.
        address: String,

        /// `server_name` to use for the Anemo network.
        #[arg(long)]
        server_name: String,
    },

    /// Sends an RPC to a peer.
    Call {
        /// Address of the target to connect to.
        address: String,

        /// `server_name` to use for the Anemo network.
        #[arg(long)]
        server_name: String,

        /// Name of the RPC service.
        service_name: String,

        /// Name of the RPC method.
        method_name: String,

        /// Body of the request to send.
        request: String,
    },
}

/// Main entrypoint to anemo CLI binaries. Call this from your main function after all
/// application-specific initialization is complete.
pub async fn main(config: Config) {
    let cli = Cli::parse();
    run(config, cli.args).await
}

/// Call this function to execute anemo CLI commands if you are embedding into an existing
/// clap binary.
pub async fn run(config: Config, args: Args) {
    match args.command {
        Commands::Ping {
            address,
            server_name,
        } => {
            ping::run(address.as_str(), server_name.as_str()).await;
        }
        Commands::Call {
            address,
            server_name,
            service_name,
            method_name,
            request,
        } => {
            call::run(
                config,
                address.as_str(),
                server_name.as_str(),
                service_name.as_str(),
                method_name.as_str(),
                request,
            )
            .await;
        }
    }
}
