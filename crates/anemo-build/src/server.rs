use crate::{
    generate_doc_comment, generate_doc_comments,
    manual::{Method, Service},
    naive_snake_case,
};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Ident, Lit, LitStr};

/// Generate service for Server.
///
/// This takes some `Service` and will generate a `TokenStream` that contains
/// a public module containing the server service and handler trait.
pub fn generate(service: &Service) -> TokenStream {
    let methods = generate_methods(service);

    let server_service = quote::format_ident!("{}Server", service.name());
    let server_trait = quote::format_ident!("{}", service.name());
    let server_mod = quote::format_ident!("{}_server", naive_snake_case(service.name()));
    let generated_trait = generate_trait(service, server_trait.clone());
    let service_doc = generate_doc_comments(service.comment());
    let package = service.package();
    let path = format!(
        "{}{}{}",
        package,
        if package.is_empty() { "" } else { "." },
        service.identifier()
    );
    let transport = generate_transport(&server_service, &server_trait, &path);

    quote! {
        /// Generated server implementations.
        pub mod #server_mod {
            #![allow(
                unused_variables,
                dead_code,
                missing_docs,
                // will trigger if compression is disabled
                clippy::let_unit_value,
            )]
            use anemo::codegen::*;

            #generated_trait

            #service_doc
            #[derive(Debug)]
            pub struct #server_service<T: #server_trait> {
                inner: Arc<T>,
            }

            impl<T: #server_trait> #server_service<T> {
                pub fn new(inner: T) -> Self {
                    Self::from_arc(Arc::new(inner))
                }

                pub fn from_arc(inner: Arc<T>) -> Self {
                    Self {
                        inner,
                    }
                }

                pub fn inner(&self) -> &T {
                    &self.inner
                }

                pub fn into_inner(self) -> Arc<T> {
                    self.inner
                }
            }

            impl<T> anemo::codegen::Service<anemo::Request<Bytes>> for #server_service<T>
                where
                    T: #server_trait,
            {
                type Response = anemo::Response<Bytes>;
                type Error = std::convert::Infallible;
                type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

                fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                    Poll::Ready(Ok(()))
                }

                fn call(&mut self, req: anemo::Request<Bytes>) -> Self::Future {
                    let inner = self.inner.clone();

                    match req.route() {
                        #methods

                        _ => Box::pin(async move {
                            Ok(anemo::types::response::StatusCode::NotFound.into_response())
                        }),
                    }
                }
            }

            impl<T: #server_trait> Clone for #server_service<T> {
                fn clone(&self) -> Self {
                    let inner = self.inner.clone();
                    Self {
                        inner,
                    }
                }
            }

            #transport
        }
    }
}

fn generate_trait(service: &Service, server_trait: Ident) -> TokenStream {
    let methods = generate_trait_methods(service);
    let trait_doc = generate_doc_comment(format!(
        "Generated trait containing RPC methods that should be implemented for use with {}Server.",
        service.name()
    ));
    let trait_attributes = service.attributes().for_trait(service.name());

    quote! {
        #trait_doc
        #(#trait_attributes)*
        #[async_trait]
        pub trait #server_trait : Send + Sync + 'static {
            #methods
        }
    }
}

fn generate_trait_methods(service: &Service) -> TokenStream {
    let mut stream = TokenStream::new();

    for method in service.methods() {
        let name = quote::format_ident!("{}", method.name());

        let request = method.request_type();
        let response = method.response_type();

        let method_doc = generate_doc_comments(method.comment());

        let method = quote! {
            #method_doc
            async fn #name(&self, request: anemo::Request<#request>)
                -> Result<anemo::Response<#response>, anemo::rpc::Status>;
        };

        stream.extend(method);
    }

    stream
}

fn generate_transport(
    server_service: &syn::Ident,
    server_trait: &syn::Ident,
    service_name: &str,
) -> TokenStream {
    let service_name = syn::LitStr::new(service_name, proc_macro2::Span::call_site());

    quote! {
        impl<T: #server_trait> anemo::rpc::RpcService for #server_service<T> {
            const SERVICE_NAME: &'static str = #service_name;
        }
    }
}

fn generate_methods(service: &Service) -> TokenStream {
    let mut stream = TokenStream::new();

    for method in service.methods() {
        let path = format!(
            "/{}{}{}/{}",
            service.package(),
            if service.package().is_empty() {
                ""
            } else {
                "."
            },
            service.identifier(),
            method.identifier()
        );
        let method_path = Lit::Str(LitStr::new(&path, Span::call_site()));
        let ident = quote::format_ident!("{}", method.name());
        let server_trait = quote::format_ident!("{}", service.name());

        let method_stream = generate_unary(method, ident, server_trait);

        let method = quote! {
            #method_path => {
                #method_stream
            }
        };
        stream.extend(method);
    }

    stream
}

fn generate_unary(method: &Method, method_ident: Ident, server_trait: Ident) -> TokenStream {
    let codec_name = syn::parse_str::<syn::Path>(method.codec_path()).unwrap();

    let service_ident = quote::format_ident!("{}Svc", method.identifier());

    let request = method.request_type();
    let response = method.response_type();

    quote! {
        #[allow(non_camel_case_types)]
        struct #service_ident<T: #server_trait >(pub Arc<T>);

        impl<T: #server_trait> anemo::rpc::server::UnaryService<#request> for #service_ident<T> {
            type Response = #response;
            type Future = BoxFuture<'static, Result<anemo::Response<Self::Response>, anemo::rpc::Status>>;

            fn call(&mut self, request: anemo::Request<#request>) -> Self::Future {
                let inner = self.0.clone();
                let fut = async move {
                    (*inner).#method_ident(request).await
                };
                Box::pin(fut)
            }
        }

        let inner = self.inner.clone();
        let fut = async move {
            let method = #service_ident(inner);
            let codec = #codec_name::default();

            let mut rpc = anemo::rpc::server::Rpc::new(codec);

            let res = rpc.unary(method, req).await;
            Ok(res)
        };

        Box::pin(fut)
    }
}
