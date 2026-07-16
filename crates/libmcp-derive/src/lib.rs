//! Derive macros for `libmcp` projection traits.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, Ident, LitStr, Result, parse_macro_input};

#[cfg(test)]
use {libmcp as _, trybuild as _};

const LIBMCP_ATTR: &str = "libmcp";
const ATTR_KIND: &str = "kind";
const ATTR_REFERENCE_ONLY: &str = "reference_only";
const ATTR_ALLOW_OPAQUE_IDS: &str = "allow_opaque_ids";
const FIELD_SELECTOR: &str = "selector";
const FIELD_TITLE: &str = "title";
const FIELD_SLUG: &str = "slug";
const FIELD_SKIP: &str = "skip";
const FIELD_SKIP_NONE: &str = "skip_none";
const FIELD_FULL_ONLY: &str = "full_only";
const FIELD_FULL: &str = "full";
const FIELD_CONCISE_ONLY: &str = "concise_only";
const DEFAULT_SURFACE_KIND: &str = "Read";

/// Derives `libmcp::StructuredProjection` and `libmcp::SurfacePolicy`.
#[proc_macro_derive(ToolProjection, attributes(libmcp))]
pub fn derive_tool_projection(input: TokenStream) -> TokenStream {
    expand_tool_projection(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `libmcp::SelectorProjection`.
#[proc_macro_derive(SelectorProjection, attributes(libmcp))]
pub fn derive_selector_projection(input: TokenStream) -> TokenStream {
    expand_selector_projection(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_tool_projection(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let data = match input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "ToolProjection only supports structs",
            ));
        }
    };
    let fields = match data.fields {
        Fields::Named(fields) => fields.named.into_iter().collect::<Vec<_>>(),
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "ToolProjection requires named fields",
            ));
        }
    };
    let container = parse_container_attrs(&input.attrs)?;
    let kind = surface_kind_expr(&container.kind);
    let reference_only = container.reference_only;
    let forbid_opaque_ids = !container.allow_opaque_ids;

    let concise_entries = fields
        .iter()
        .filter(|field| include_in_concise(field))
        .map(project_field)
        .collect::<Result<Vec<_>>>()?;
    let full_entries = fields
        .iter()
        .filter(|field| include_in_full(field))
        .map(project_field)
        .collect::<Result<Vec<_>>>()?;

    Ok(quote! {
        impl ::libmcp::StructuredProjection for #ident {
            fn concise_projection(
                &self,
            ) -> ::std::result::Result<::serde_json::Value, ::libmcp::ProjectionError> {
                let mut object = ::serde_json::Map::new();
                #(#concise_entries)*
                Ok(::serde_json::Value::Object(object))
            }

            fn full_projection(
                &self,
            ) -> ::std::result::Result<::serde_json::Value, ::libmcp::ProjectionError> {
                let mut object = ::serde_json::Map::new();
                #(#full_entries)*
                Ok(::serde_json::Value::Object(object))
            }
        }

        impl ::libmcp::SurfacePolicy for #ident {
            const KIND: ::libmcp::SurfaceKind = #kind;
            const FORBID_OPAQUE_IDS: bool = #forbid_opaque_ids;
            const REFERENCE_ONLY: bool = #reference_only;
        }
    })
}

fn expand_selector_projection(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let data = match input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "SelectorProjection only supports structs",
            ));
        }
    };
    let fields = match data.fields {
        Fields::Named(fields) => fields.named.into_iter().collect::<Vec<_>>(),
        _ => {
            return Err(syn::Error::new_spanned(
                ident,
                "SelectorProjection requires named fields",
            ));
        }
    };

    let slug_field = fields
        .iter()
        .find(|field| has_field_flag(field, FIELD_SELECTOR) || field_ident(field) == FIELD_SLUG)
        .ok_or_else(|| syn::Error::new_spanned(&ident, "SelectorProjection needs a slug field"))?;
    let title_field = fields
        .iter()
        .find(|field| has_field_flag(field, FIELD_TITLE) || field_ident(field) == FIELD_TITLE);

    let slug_ident = slug_field.ident.as_ref().ok_or_else(|| {
        syn::Error::new_spanned(slug_field, "SelectorProjection needs named fields")
    })?;
    let title_tokens = if let Some(field) = title_field {
        let title_ident = field.ident.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(field, "SelectorProjection needs named fields")
        })?;
        quote! { title: ::std::option::Option::Some(self.#title_ident.clone().into()) }
    } else {
        quote! { title: ::std::option::Option::None }
    };

    Ok(quote! {
        impl ::libmcp::SelectorProjection for #ident {
            fn selector_ref(&self) -> ::libmcp::SelectorRef {
                ::libmcp::SelectorRef {
                    slug: self.#slug_ident.clone(),
                    #title_tokens,
                }
            }
        }
    })
}

fn project_field(field: &Field) -> Result<proc_macro2::TokenStream> {
    let ident = field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(field, "ToolProjection requires named fields"))?;
    let key = LitStr::new(field_ident(field).as_str(), ident.span());
    if has_field_flag(field, FIELD_SKIP_NONE) {
        Ok(quote! {
            if let ::std::option::Option::Some(value) = &self.#ident {
                object.insert(
                    #key.to_owned(),
                    ::serde_json::to_value(value).map_err(::libmcp::ProjectionError::from)?,
                );
            }
        })
    } else {
        Ok(quote! {
            object.insert(
                #key.to_owned(),
                ::serde_json::to_value(&self.#ident).map_err(::libmcp::ProjectionError::from)?,
            );
        })
    }
}

fn include_in_concise(field: &Field) -> bool {
    !has_field_flag(field, FIELD_SKIP)
        && !has_field_flag(field, FIELD_FULL_ONLY)
        && !has_field_flag(field, FIELD_FULL)
}

fn include_in_full(field: &Field) -> bool {
    !has_field_flag(field, FIELD_SKIP) && !has_field_flag(field, FIELD_CONCISE_ONLY)
}

fn field_ident(field: &Field) -> String {
    field
        .ident
        .as_ref()
        .map_or_else(String::new, ToString::to_string)
}

#[derive(Default)]
struct ContainerAttrs {
    kind: Option<Ident>,
    reference_only: bool,
    allow_opaque_ids: bool,
}

fn parse_container_attrs(attrs: &[syn::Attribute]) -> Result<ContainerAttrs> {
    let mut parsed = ContainerAttrs::default();
    for attr in attrs
        .iter()
        .filter(|attr| attr.path().is_ident(LIBMCP_ATTR))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(ATTR_KIND) {
                let value = meta.value()?;
                let kind = value.parse::<LitStr>()?;
                parsed.kind = Some(Ident::new(
                    normalize_surface_kind(kind.value()).as_str(),
                    kind.span(),
                ));
                return Ok(());
            }
            if meta.path.is_ident(ATTR_REFERENCE_ONLY) {
                parsed.reference_only = true;
                return Ok(());
            }
            if meta.path.is_ident(ATTR_ALLOW_OPAQUE_IDS) {
                parsed.allow_opaque_ids = true;
                return Ok(());
            }
            Err(meta.error("unsupported libmcp container attribute"))
        })?;
    }
    Ok(parsed)
}

fn has_field_flag(field: &Field, flag: &str) -> bool {
    field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident(LIBMCP_ATTR))
        .any(|attr| attr_has_flag(attr, flag))
}

fn attr_has_flag(attr: &syn::Attribute, flag: &str) -> bool {
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident(flag) {
            found = true;
            return Ok(());
        }
        Ok(())
    });
    found
}

fn surface_kind_expr(kind: &Option<Ident>) -> proc_macro2::TokenStream {
    let ident = kind
        .clone()
        .unwrap_or_else(|| Ident::new(DEFAULT_SURFACE_KIND, proc_macro2::Span::call_site()));
    quote!(::libmcp::SurfaceKind::#ident)
}

fn normalize_surface_kind(kind: String) -> String {
    let normalized = kind.trim().replace('-', "_");
    let mut out = String::new();
    let mut uppercase_next = true;
    for ch in normalized.chars() {
        if ch == '_' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}
