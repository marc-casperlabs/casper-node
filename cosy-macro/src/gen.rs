use crate::{
    parse::{ReactorDefinition, Target},
    util::suffix_ident,
};
use proc_macro2::TokenStream;
use syn::export::quote::quote;

/// Generates the top level reactor `struct`.
///
/// Will generate a field for each component to be used.
pub(crate) fn generate_reactor(def: &ReactorDefinition) -> TokenStream {
    let reactor_ident = def.reactor_ident();

    let mut reactor_fields = Vec::new();

    for component in def.components() {
        let field_name = component.field_ident();
        let full_type = component.full_component_type();

        reactor_fields.push(quote!(#field_name: #full_type));
    }

    quote!(
        #[derive(Debug)]
        pub struct #reactor_ident {
            #(#reactor_fields,)*
        }
    )
}

/// Generates types for the reactor implementation.
pub(crate) fn generate_reactor_types(def: &ReactorDefinition) -> TokenStream {
    let reactor_ident = def.reactor_ident();
    let event_ident = suffix_ident(&reactor_ident, "Event");
    let error_ident = suffix_ident(&reactor_ident, "Error");

    let mut event_variants = Vec::new();
    let mut error_variants = Vec::new();
    let mut display_variants = Vec::new();
    let mut error_display_variants = Vec::new();
    let mut error_source_variants = Vec::new();
    let mut from_impls = Vec::new();

    for component in def.components() {
        let variant_ident = component.variant_ident();
        let full_event_type = def.component_event(component);
        let full_error_type = component.full_error_type(quote!(#event_ident));
        let field_name = component.field_ident().to_string();

        event_variants.push(quote!(#variant_ident(#full_event_type)));
        error_variants.push(quote!(#variant_ident(#full_error_type)));

        display_variants.push(quote!(
           #event_ident::#variant_ident(inner) => write!(f, "{}: {}", #field_name, inner)
        ));

        error_display_variants.push(quote!(
           #error_ident::#variant_ident(inner) => write!(f, "{}: {}", #field_name, inner)
        ));

        error_source_variants.push(quote!(
            #error_ident::#variant_ident(inner) => Some(inner)
        ));

        from_impls.push(quote!(
            impl From<#full_event_type> for #event_ident {
                fn from(event: #full_event_type) -> Self {
                    #event_ident::#variant_ident(event)
                }
            }
        ));
    }

    // NOTE: Cannot use `From::from` to directly construct next component's event because doing so
    //       prevents us from implementing discards.

    // Add a variant for each request and a `From` implementation.
    for request in def.requests() {
        let variant_ident = request.variant_ident();
        let full_request_type = request.full_request_type();

        event_variants.push(quote!(#variant_ident(#full_request_type)));

        display_variants.push(quote!(
           #event_ident::#variant_ident(inner) => ::std::fmt::Display::fmt(inner, f)
        ));

        from_impls.push(quote!(
            impl From<#full_request_type> for #event_ident {
                fn from(request: #full_request_type) -> Self {
                    #event_ident::#variant_ident(request)
                }
            }
        ));
    }

    quote!(
        #[derive(Debug)]
        pub enum #event_ident {
           #(#event_variants,)*
        }

        #[derive(Debug)]
        pub enum #error_ident {
            #(#error_variants,)*
            MetricsInitialization(prometheus::Error),
        }

        impl std::fmt::Display for #event_ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #(#display_variants,)*
                }
            }
        }

        impl std::fmt::Display for #error_ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #(#error_display_variants,)*
                    #error_ident::MetricsInitialization(err) => write!(f, "metrics_initialization: {}", err),
                }
            }
        }

        #(#from_impls)*

        impl From<prometheus::Error> for #error_ident {
            fn from(err: prometheus::Error) -> Self {
                #error_ident::MetricsInitialization(err)
            }
        }

        impl std::error::Error for #error_ident {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                    #(#error_source_variants,)*
                    #error_ident::MetricsInitialization(inner) => Some(inner),
                }
            }
        }
    )
}

/// Generates the reactor implementation itself.
pub(crate) fn generate_reactor_impl(def: &ReactorDefinition) -> TokenStream {
    let reactor_ident = def.reactor_ident();
    let event_ident = def.event_ident();
    let error_ident = def.error_ident();
    let config = def.config_type().as_given();

    let mut dispatches = Vec::new();

    // Generate dispatches for component events.
    for component in def.components() {
        let variant_ident = component.variant_ident();
        let full_component_type = component.full_component_type();
        let field_ident = component.field_ident();

        dispatches.push(quote!(
            #event_ident::#variant_ident(event) => {
                crate::reactor::wrap_effects(
                    #event_ident::#variant_ident,
                    <#full_component_type as crate::components::Component<#event_ident>>::handle_event(&mut self.#field_ident, effect_builder, rng, event)
                )
            },
        ));

        for request in def.requests() {
            let request_variant_ident = request.variant_ident();

            match request.target() {
                Target::Discard => {
                    dispatches.push(quote!(
                        #event_ident::#request_variant_ident(request) => {
                            // Request is discarded.
                            // TODO: Add `trace!` call here? Consider the log spam though.
                            Default::default()
                        },
                    ));
                }
                Target::Dest(ref dest) => {
                    let dest_component_type = def.component(dest).full_component_type();
                    let dest_variant_ident = def.component(dest).variant_ident();
                    let dest_field_ident = dest;

                    dispatches.push(quote!(
                        #event_ident::#request_variant_ident(request) => {
                            // Turn request into event for target component.
                            let dest_event = <#dest_component_type as crate::components::Component<Self::Event>>::Event::from(request);

                            // Route the newly created event to the component.
                            crate::reactor::wrap_effects(
                                #event_ident::#dest_variant_ident,
                                <#dest_component_type as crate::components::Component<Self::Event>>::handle_event(&mut self.#dest_field_ident, effect_builder, rng, dest_event)
                            )
                        },
                    ));
                }
            }
        }
    }

    let mut component_instantiations = Vec::new();
    let mut component_fields = Vec::new();

    for cdef in def.components() {
        let field_ident = cdef.field_ident();
        let component_type = cdef.full_component_type();
        let variant_ident = cdef.variant_ident();

        let constructor_args = cdef.component_arguments();

        component_instantiations.push(quote!(
            let (#field_ident, effects) = #component_type::new(#(#constructor_args),*)
                .map_err(#error_ident::#variant_ident)?;
        ));

        component_fields.push(quote!(#field_ident));
    }

    quote!(
        impl crate::reactor::Reactor for #reactor_ident {
            type Event = #event_ident;
            type Error = #error_ident;
            type Config = #config;

            fn dispatch_event(
                &mut self,
                effect_builder: crate::reactor::EffectBuilder<Self::Event>,
                rng: &mut dyn crate::types::CryptoRngCore,
                event: Self::Event,
            ) -> crate::reactor::Effects<Self::Event> {
                match event {
                    #(#dispatches)*
                }
            }

            fn new(
                cfg: Self::Config,
                registry: &crate::reactor::Registry,
                event_queue: crate::reactor::EventQueueHandle<Self::Event>,
                rng: &mut dyn crate::types::CryptoRngCore,
            ) -> Result<(Self, crate::reactor::Effects<Self::Event>), Self::Error> {
                // Instantiate each component.
                #(#component_instantiations)*

                // Assign component fields during reactor construction.
                Ok(#reactor_ident {
                    #(#component_fields,)*
                })
            }
        }
    )
}