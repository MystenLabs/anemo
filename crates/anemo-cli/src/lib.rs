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

    pub fn add_service(mut self, name: String, info: ServiceInfo) -> Self {
        assert!(
            self.service_map.insert(name, info).is_none(),
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

    pub fn add_method(mut self, name: String, method_fn: MethodFn) -> Self {
        assert!(
            self.method_map.insert(name, method_fn).is_none(),
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

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Connects to the indicated peer without doing anything.
    Ping {
        /// Address of the target to connect to.
        address: Box<str>,

        /// `server_name` to use for the Anemo network.
        #[arg(long)]
        server_name: String,
    },

    /// Sends an RPC to a peer.
    Call {
        /// Address of the target to connect to.
        address: Box<str>,

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

    match cli.command {
        Commands::Ping {
            address,
            server_name,
        } => {
            ping::run(address, server_name).await;
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
                address,
                server_name,
                service_name,
                method_name,
                request,
            )
            .await;
        }
    }
}
