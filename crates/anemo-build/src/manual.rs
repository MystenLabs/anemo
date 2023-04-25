//! This module provides utilities for generating `anemo` service stubs and clients.
//!
//! # Example
//!
//! ```rust,no_run
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let greeter_service = anemo_build::manual::Service::builder()
//!         .name("Greeter")
//!         .package("helloworld")
//!         .method(
//!             anemo_build::manual::Method::builder()
//!                 .name("say_hello")
//!                 .route_name("SayHello")
//!                 // Provide the path to the Request type
//!                 .request_type("crate::HelloRequest")
//!                 // Provide the path to the Response type
//!                 .response_type("super::HelloResponse")
//!                 // Provide the path to the Codec to use
//!                 .codec_path("crate::JsonCodec")
//!                 .build(),
//!         )
//!         .build();
//!
//!     anemo_build::manual::Builder::new().compile(&[greeter_service]);
//!     Ok(())
//! }
//! ```

use super::{client, server, Attributes};
use proc_macro2::TokenStream;
use quote::ToTokens;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Service builder.
///
/// This builder can be used to manually define an RPC service.
///
/// # Example
///
/// ```
/// # use anemo_build::manual::Service;
/// let greeter_service = Service::builder()
///     .name("Greeter")
///     .package("helloworld")
///     // Add various methods to the service
///     // .method()
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct ServiceBuilder {
    /// The service name in Rust style.
    name: Option<String>,
    /// The package name.
    package: Option<String>,
    /// The service comments.
    comments: Vec<String>,
    /// The service methods.
    methods: Vec<Method>,
    /// Attributes to apply to generated code.
    attributes: Attributes,
}

impl ServiceBuilder {
    /// Set the name for this Service.
    ///
    /// This value will be used both as the base for the generated rust types and service trait as
    /// well as part of the route for calling this service. Routes have the form:
    /// `/<package_name>.<service_name>/<method_route_name>`
    pub fn name(mut self, name: impl AsRef<str>) -> Self {
        self.name = Some(name.as_ref().to_owned());
        self
    }

    /// Set the package this Service is part of.
    ///
    /// This value will be used as part of the route for calling this service.
    /// Routes have the form: `/<package_name>.<service_name>/<method_route_name>`
    pub fn package(mut self, package: impl AsRef<str>) -> Self {
        self.package = Some(package.as_ref().to_owned());
        self
    }

    /// Add a comment string that should be included as a doc comment for this Service.
    pub fn comment(mut self, comment: impl AsRef<str>) -> Self {
        self.comments.push(comment.as_ref().to_owned());
        self
    }

    /// Adds a Method to this Service.
    pub fn method(mut self, method: Method) -> Self {
        self.methods.push(method);
        self
    }

    /// Adds Attributes which should be applied to items in the generated code.
    pub fn attributes(mut self, attributes: Attributes) -> Self {
        self.attributes = attributes;
        self
    }

    /// Build a Service.
    ///
    /// Panics if `name` wasn't set.
    pub fn build(self) -> Service {
        Service {
            name: self.name.unwrap(),
            comments: self.comments,
            package: self.package.unwrap_or_default(),
            methods: self.methods,
            attributes: self.attributes,
        }
    }
}

/// A service descriptor.
#[derive(Debug)]
pub struct Service {
    /// The service name in Rust style.
    name: String,
    /// The package name.
    package: String,
    /// The service comments.
    comments: Vec<String>,
    /// The service methods.
    methods: Vec<Method>,
    /// Attributes to apply to generated code.
    attributes: Attributes,
}

impl Service {
    /// Create a new `ServiceBuilder`
    pub fn builder() -> ServiceBuilder {
        ServiceBuilder::default()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn identifier(&self) -> &str {
        &self.name
    }

    pub fn methods(&self) -> &[Method] {
        &self.methods
    }

    pub fn comment(&self) -> &[String] {
        &self.comments
    }

    pub fn attributes(&self) -> &Attributes {
        &self.attributes
    }
}

/// A service method descriptor.
#[derive(Debug)]
pub struct Method {
    /// The name of the method in Rust style.
    name: String,
    /// The name of the method as should be used when constructing a route
    route_name: String,
    /// The method comments.
    comments: Vec<String>,
    /// The input Rust type.
    request_type: String,
    /// The output Rust type.
    response_type: String,
    /// The path to the codec to use for this method
    codec_path: String,
    /// Use raw (serialized) bytes for the server-side response handler.
    server_handler_return_raw_bytes: bool,
}

impl Method {
    /// Create a new `MethodBuilder`
    pub fn builder() -> MethodBuilder {
        MethodBuilder::default()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn identifier(&self) -> &str {
        &self.route_name
    }

    pub fn codec_path(&self) -> &str {
        &self.codec_path
    }

    pub fn comment(&self) -> &[String] {
        &self.comments
    }

    pub fn request_type(&self) -> TokenStream {
        syn::parse_str::<syn::Type>(&self.request_type)
            .unwrap()
            .to_token_stream()
    }

    pub fn response_type(&self) -> TokenStream {
        syn::parse_str::<syn::Type>(&self.response_type)
            .unwrap()
            .to_token_stream()
    }

    pub fn server_handler_return_raw_bytes(&self) -> bool {
        self.server_handler_return_raw_bytes
    }
}

/// Method builder.
///
/// This builder can be used to manually define an RPC method, which can be added to an RPC service.
///
/// # Example
///
/// ```
/// # use anemo_build::manual::Method;
/// let say_hello_method = Method::builder()
///     .name("say_hello")
///     .route_name("SayHello")
///     // Provide the path to the Request type
///     .request_type("crate::common::HelloRequest")
///     // Provide the path to the Response type
///     .response_type("crate::common::HelloResponse")
///     // Provide the path to the Codec to use
///     .codec_path("crate::common::JsonCodec")
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct MethodBuilder {
    /// The name of the method in Rust style.
    name: Option<String>,
    /// The name of the method as should be used when constructing a route
    route_name: Option<String>,
    /// The method comments.
    comments: Vec<String>,
    /// The input Rust type.
    request_type: Option<String>,
    /// The output Rust type.
    response_type: Option<String>,
    /// The path to the codec to use for this method
    codec_path: Option<String>,
    /// Use raw (serialized) bytes for the server-side response handler.
    server_handler_return_raw_bytes: bool,
}

impl MethodBuilder {
    /// Set the name for this Method.
    ///
    /// This value will be used for generating the client functions for calling this Method.
    ///
    /// Generally this is formatted in snake_case.
    pub fn name(mut self, name: impl AsRef<str>) -> Self {
        self.name = Some(name.as_ref().to_owned());
        self
    }

    /// Set the route_name for this Method.
    ///
    /// This value will be used as part of the route for calling this method.
    /// Routes have the form: `/<package_name>.<service_name>/<method_route_name>`
    ///
    /// Generally this is formatted in PascalCase.
    pub fn route_name(mut self, route_name: impl AsRef<str>) -> Self {
        self.route_name = Some(route_name.as_ref().to_owned());
        self
    }

    /// Add a comment string that should be included as a doc comment for this Method.
    pub fn comment(mut self, comment: impl AsRef<str>) -> Self {
        self.comments.push(comment.as_ref().to_owned());
        self
    }

    /// Set the path to the Rust type that should be use for the Request type of this method.
    pub fn request_type(mut self, request_type: impl AsRef<str>) -> Self {
        self.request_type = Some(request_type.as_ref().to_owned());
        self
    }

    /// Set the path to the Rust type that should be use for the Response type of this method.
    pub fn response_type(mut self, response_type: impl AsRef<str>) -> Self {
        self.response_type = Some(response_type.as_ref().to_owned());
        self
    }

    /// Set the path to the Rust type that should be used as the `Codec` for this method.
    ///
    /// Currently the codegen assumes that this type implements `Default`.
    pub fn codec_path(mut self, codec_path: impl AsRef<str>) -> Self {
        self.codec_path = Some(codec_path.as_ref().to_owned());
        self
    }

    /// Set whether or not the server handler should use raw bytes for the response.
    pub fn server_handler_return_raw_bytes(mut self, use_raw_bytes: bool) -> Self {
        self.server_handler_return_raw_bytes = use_raw_bytes;
        self
    }

    /// Build a Method
    ///
    /// Panics if `name`, `route_name`, `request_type`, `response_type`, or `codec_path` weren't set.
    pub fn build(self) -> Method {
        Method {
            name: self.name.unwrap(),
            route_name: self.route_name.unwrap(),
            comments: self.comments,
            request_type: self.request_type.unwrap(),
            response_type: self.response_type.unwrap(),
            codec_path: self.codec_path.unwrap(),
            server_handler_return_raw_bytes: self.server_handler_return_raw_bytes,
        }
    }
}

struct ServiceGenerator {
    builder: Builder,
    clients: TokenStream,
    servers: TokenStream,
}

impl ServiceGenerator {
    fn generate(&mut self, service: &Service) {
        if self.builder.build_server {
            let server = server::generate(service);
            self.servers.extend(server);
        }

        if self.builder.build_client {
            let client = client::generate(service);
            self.clients.extend(client);
        }
    }

    fn finalize(&mut self, buf: &mut String) {
        if self.builder.build_client && !self.clients.is_empty() {
            let clients = &self.clients;

            let client_service = quote::quote! {
                #clients
            };

            let ast: syn::File = syn::parse2(client_service).expect("not a valid tokenstream");
            let code = prettyplease::unparse(&ast);
            buf.push_str(&code);

            self.clients = TokenStream::default();
        }

        if self.builder.build_server && !self.servers.is_empty() {
            let servers = &self.servers;

            let server_service = quote::quote! {
                #servers
            };

            let ast: syn::File = syn::parse2(server_service).expect("not a valid tokenstream");
            let code = prettyplease::unparse(&ast);
            buf.push_str(&code);

            self.servers = TokenStream::default();
        }
    }
}

/// Service generator builder.
#[derive(Debug)]
pub struct Builder {
    build_server: bool,
    build_client: bool,

    out_dir: Option<PathBuf>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            build_server: true,
            build_client: true,
            out_dir: None,
        }
    }
}

impl Builder {
    /// Create a new Builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable RPC client code generation.
    ///
    /// Defaults to enabling client code generation.
    pub fn build_client(mut self, enable: bool) -> Self {
        self.build_client = enable;
        self
    }

    /// Enable or disable RPC server code generation.
    ///
    /// Defaults to enabling server code generation.
    pub fn build_server(mut self, enable: bool) -> Self {
        self.build_server = enable;
        self
    }

    /// Set the output directory to generate code to.
    ///
    /// Defaults to the `OUT_DIR` environment variable.
    pub fn out_dir(mut self, out_dir: impl AsRef<Path>) -> Self {
        self.out_dir = Some(out_dir.as_ref().to_path_buf());
        self
    }

    /// Performs code generation for the provided services.
    ///
    /// Generated services will be output into the directory specified by `out_dir`
    /// with files named `<package_name>.<service_name>.rs`.
    pub fn compile(self, services: &[Service]) {
        let out_dir = if let Some(out_dir) = self.out_dir.as_ref() {
            out_dir.clone()
        } else {
            PathBuf::from(std::env::var("OUT_DIR").unwrap())
        };

        let mut generator = ServiceGenerator {
            builder: self,
            clients: TokenStream::default(),
            servers: TokenStream::default(),
        };

        for service in services {
            generator.generate(service);
            let mut output = String::new();
            generator.finalize(&mut output);

            let out_file = out_dir.join(format!(
                "{}{}{}.rs",
                service.package,
                if service.package.is_empty() { "" } else { "." },
                service.name
            ));
            fs::write(out_file, output).unwrap();
        }
    }
}
