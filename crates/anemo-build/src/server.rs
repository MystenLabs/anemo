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
    let method_routes = generate_method_routes(service);
    let method_services = generate_method_services(service);

    let method_layer_names: Vec<_> = service
        .methods()
        .iter()
        .map(|method| quote::format_ident!("{}_layer", method.name()))
        .collect();
    let method_request_types: Vec<_> = service
        .methods()
        .iter()
        .map(|method| method.request_type())
        .collect();
    let method_response_types: Vec<_> = service
        .methods()
        .iter()
        .map(|method| {
            if method.server_handler_return_raw_bytes() {
                quote! { bytes::Bytes }
            } else {
                method.response_type()
            }
        })
        .collect();
    let add_layer_function_names: Vec<_> = service
        .methods()
        .iter()
        .map(|method| quote::format_ident!("add_layer_for_{}", method.name()))
        .collect();

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
            pub use anemo::codegen::InboundRequestLayer;

            #generated_trait

            #service_doc
            #[derive(Debug)]
            pub struct #server_service<T: #server_trait> {
                inner: Arc<T>,
                #( #method_layer_names: InboundRequestLayer<#method_request_types, #method_response_types>, )*
            }

            impl<T: #server_trait> #server_service<T> {
                pub fn new(inner: T) -> Self {
                    Self::from_arc(Arc::new(inner))
                }

                pub fn from_arc(inner: Arc<T>) -> Self {
                    Self {
                        inner,
                        #( #method_layer_names: InboundRequestLayer::new(Identity::new()), )*
                    }
                }

                pub fn inner(&self) -> &T {
                    &self.inner
                }

                pub fn into_inner(self) -> Arc<T> {
                    self.inner
                }

                #(
                pub fn #add_layer_function_names(
                    mut self,
                    layer: InboundRequestLayer<#method_request_types, #method_response_types>,
                ) -> Self {
                    self.#method_layer_names = InboundRequestLayer::new(
                        Stack::new(self.#method_layer_names, layer)
                    );
                    self
                }
                )*
            }

            #method_services

            impl<T> anemo::codegen::Service<anemo::Request<Bytes>> for #server_service<T>
                where
                    T: #server_trait,
            {
                type Response = anemo::Response<Bytes>;
                type Error = std::convert::Infallible;
                type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

                fn poll_ready(
                    &mut self,
                    _cx: &mut Context<'_>,
                ) -> Poll<Result<(), Self::Error>> {
                    Poll::Ready(Ok(()))
                }

                fn call(&mut self, req: anemo::Request<Bytes>) -> Self::Future {
                    let inner = self.inner.clone();

                    match req.route() {
                        #method_routes

                        _ => Box::pin(async move {
                            Ok(anemo::types::response::StatusCode::NotFound.into_response())
                        }),
                    }
                }
            }

            impl<T: #server_trait> Clone for #server_service<T> {
                fn clone(&self) -> Self {
                    let inner = self.inner.clone();
                    #( let #method_layer_names = self.#method_layer_names.clone(); )*
                    Self {
                        inner,
                        #(#method_layer_names),*
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
        let response = if method.server_handler_return_raw_bytes() {
            quote! { bytes::Bytes }
        } else {
            method.response_type()
        };

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

fn generate_method_services(service: &Service) -> TokenStream {
    let mut stream = TokenStream::new();

    for method in service.methods() {
        let server_trait = quote::format_ident!("{}", service.name());
        let method_service = generate_method_service(method, server_trait);
        stream.extend(method_service);
    }

    stream
}

fn generate_method_service(method: &Method, server_trait: Ident) -> TokenStream {
    let method_ident = quote::format_ident!("{}", method.name());
    let service_ident = quote::format_ident!("{}Svc", method.identifier());

    let request = method.request_type();
    let response = if method.server_handler_return_raw_bytes() {
        quote! { bytes::Bytes }
    } else {
        method.response_type()
    };

    quote! {
        #[allow(non_camel_case_types)]
        #[derive(Debug)]
        struct #service_ident<T: #server_trait >(pub Arc<T>);

        impl<T: #server_trait> anemo::codegen::Service<anemo::Request<#request>> for #service_ident<T> {
            type Response = anemo::Response<#response>;
            type Error = anemo::rpc::Status;
            type Future = BoxFuture<'static, Result<Self::Response, anemo::rpc::Status>>;

            fn poll_ready(
                &mut self,
                cx: &mut core::task::Context<'_>,
            ) -> core::task::Poll<Result<(), Self::Error>> {
                core::task::Poll::Ready(Ok(()))
            }

            fn call(&mut self, request: anemo::Request<#request>) -> Self::Future {
                let inner = self.0.clone();
                let fut = async move {
                    (*inner).#method_ident(request).await
                };
                Box::pin(fut)
            }
        }

        impl<T: #server_trait> Clone for #service_ident<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }
    }
}

fn generate_method_routes(service: &Service) -> TokenStream {
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
        let method_stream = generate_method_route(method);

        let method = quote! {
            #method_path => {
                #method_stream
            }
        };
        stream.extend(method);
    }

    stream
}

fn generate_method_route(method: &Method) -> TokenStream {
    let codec_name = syn::parse_str::<syn::Path>(method.codec_path()).unwrap();

    let response_type = method.response_type();
    let request_type = method.request_type();

    let layer_name = quote::format_ident!("{}_layer", method.name());
    let service_ident = quote::format_ident!("{}Svc", method.identifier());

    let codec_init = if method.server_handler_return_raw_bytes() {
        quote! {
            let request_codec = #codec_name::<#response_type, #request_type>::default();
            let response_codec =
                anemo::rpc::codec::IdentityCodec::new(request_codec.format_name());
        }
    } else {
        quote! {
            let request_codec = #codec_name::<#response_type, #request_type>::default();
            let response_codec = #codec_name::<#response_type, #request_type>::default();
        }
    };

    quote! {
        let inner = self.inner.clone();
        let layer = self.#layer_name.clone();
        let fut = async move {
            let method = layer.layer(BoxCloneService::new(#service_ident(inner)));
            #codec_init

            let mut rpc = anemo::rpc::server::Rpc::new(request_codec, response_codec);

            let res = rpc.unary(method, req).await;
            Ok(res)
        };

        Box::pin(fut)
    }
}
