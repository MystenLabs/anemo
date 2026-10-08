use crate::{
    generate_doc_comments,
    manual::{Method, Service},
    naive_snake_case,
};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// Generate service for client.
///
/// This takes some `Service` and will generate a `TokenStream` that contains
/// a public module with the generated client.
pub fn generate(service: &Service) -> TokenStream {
    let service_ident = quote::format_ident!("{}Client", service.name());
    let client_mod = quote::format_ident!("{}_client", naive_snake_case(service.name()));
    let methods = generate_methods(service);

    let service_doc = generate_doc_comments(service.comment());

    quote! {
        /// Generated client implementations.
        pub mod #client_mod {
            #![allow(
                unused_variables,
                dead_code,
                missing_docs,
                // will trigger if compression is disabled
                clippy::let_unit_value,
                // generated methods return `anemo::rpc::Status` by value
                clippy::result_large_err,
            )]
            use anemo::codegen::*;

            #service_doc
            #[derive(Debug, Clone)]
            pub struct #service_ident<T> {
                inner: anemo::rpc::client::Rpc<T>,
            }

            impl<T> #service_ident<T>
            where
                T: Service<Request<Bytes>, Response = Response<Bytes>>,
                T::Error: Into<BoxError>,
            {
                pub fn new(inner: T) -> Self {
                    let inner = anemo::rpc::client::Rpc::new(inner);
                    Self { inner }
                }

                /// Gets a reference to the underlying service.
                pub fn inner(&self) -> &T {
                    self.inner.inner()
                }

                /// Gets a mutable reference to the underlying service.
                pub fn inner_mut(&mut self) -> &mut T {
                    self.inner.inner_mut()
                }

                /// Consumes `self`, returning the underlying service.
                pub fn into_inner(self) -> T {
                    self.inner.into_inner()
                }

                #methods
            }
        }
    }
}

fn generate_methods(service: &Service) -> TokenStream {
    let mut stream = TokenStream::new();
    let package = service.package();

    for method in service.methods() {
        let path = format!(
            "/{}{}{}/{}",
            package,
            if package.is_empty() { "" } else { "." },
            service.identifier(),
            method.identifier()
        );

        stream.extend(generate_doc_comments(method.comment()));

        let method = generate_unary(method, path);

        stream.extend(method);
    }

    stream
}

fn generate_unary(method: &Method, path: String) -> TokenStream {
    let codec_name = syn::parse_str::<syn::Path>(method.codec_path()).unwrap();
    let ident = format_ident!("{}", method.name());
    let request = method.request_type();
    let response = method.response_type();

    quote! {
        pub async fn #ident(
            &mut self,
            request: impl anemo::types::request::IntoRequest<#request>,
        ) -> Result<anemo::Response<#response>, anemo::rpc::Status> {
           self.inner.ready().await.map_err(|e| {
               anemo::rpc::Status::unknown(format!("Service was not ready: {}", e.into()))
           })?;
           let codec = #codec_name::default();
           let mut request = request.into_request();
           *request.route_mut() = #path.into();
           self.inner.unary(request, codec).await
        }
    }
}
