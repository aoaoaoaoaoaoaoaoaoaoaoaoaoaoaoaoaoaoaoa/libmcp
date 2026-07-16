//! Derive macros for `libmcp` projection traits.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use std::collections::BTreeSet;
use syn::{
    Data, DeriveInput, Field, Fields, Ident, LitStr, Result, Type, ext::IdentExt,
    meta::ParseNestedMeta, parse_macro_input, parse_quote,
};

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
    let DeriveInput {
        attrs,
        ident,
        mut generics,
        data,
        ..
    } = input;
    let fields = named_fields(data, &ident, "ToolProjection")?;
    let container = parse_container_attrs(&attrs)?;
    let parsed_fields = fields
        .iter()
        .map(|field| parse_field_attrs(field).map(|attrs| (field, attrs)))
        .collect::<Result<Vec<_>>>()?;
    let libmcp = libmcp_path()?;

    for (field, attrs) in &parsed_fields {
        if attrs.projected() {
            let ty = &field.ty;
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#ty: #libmcp::__macro::serde::Serialize));
        }
    }

    let concise_entries = parsed_fields
        .iter()
        .filter(|(_, attrs)| attrs.in_concise())
        .map(|(field, attrs)| project_field(field, attrs, &libmcp))
        .collect::<Result<Vec<_>>>()?;
    let full_entries = parsed_fields
        .iter()
        .filter(|(_, attrs)| attrs.in_full())
        .map(|(field, attrs)| project_field(field, attrs, &libmcp))
        .collect::<Result<Vec<_>>>()?;
    let kind = container.kind.tokens(&libmcp);
    let reference_only = container.reference_only;
    let forbid_opaque_ids = !container.allow_opaque_ids;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #libmcp::StructuredProjection for #ident #ty_generics #where_clause {
            fn concise_projection(
                &self,
            ) -> ::std::result::Result<
                #libmcp::__macro::serde_json::Value,
                #libmcp::ProjectionError,
            > {
                let mut object = #libmcp::__macro::serde_json::Map::new();
                #(#concise_entries)*
                Ok(#libmcp::__macro::serde_json::Value::Object(object))
            }

            fn full_projection(
                &self,
            ) -> ::std::result::Result<
                #libmcp::__macro::serde_json::Value,
                #libmcp::ProjectionError,
            > {
                let mut object = #libmcp::__macro::serde_json::Map::new();
                #(#full_entries)*
                Ok(#libmcp::__macro::serde_json::Value::Object(object))
            }
        }

        impl #impl_generics #libmcp::SurfacePolicy for #ident #ty_generics #where_clause {
            fn projection_policy(&self) -> #libmcp::ProjectionPolicy {
                #libmcp::ProjectionPolicy::from_surface(
                    #kind,
                    #forbid_opaque_ids,
                    #reference_only,
                )
            }
        }
    })
}

fn expand_selector_projection(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let DeriveInput {
        attrs,
        ident,
        mut generics,
        data,
        ..
    } = input;
    let _container = parse_container_attrs(&attrs)?;
    let fields = named_fields(data, &ident, "SelectorProjection")?;
    let parsed_fields = fields
        .iter()
        .map(|field| parse_field_attrs(field).map(|attrs| (field, attrs)))
        .collect::<Result<Vec<_>>>()?;
    let libmcp = libmcp_path()?;

    let slug_field = unique_marked_field(
        &parsed_fields,
        |attrs| attrs.has(FieldFlag::Selector),
        "selector",
    )?
    .or_else(|| {
        parsed_fields
            .iter()
            .find(|(field, _)| field_ident(field) == FIELD_SLUG)
            .map(|(field, _)| *field)
    })
    .ok_or_else(|| syn::Error::new_spanned(&ident, "SelectorProjection needs a slug field"))?;
    let title_field =
        unique_marked_field(&parsed_fields, |attrs| attrs.has(FieldFlag::Title), "title")?.or_else(
            || {
                parsed_fields
                    .iter()
                    .find(|(field, _)| field_ident(field) == FIELD_TITLE)
                    .map(|(field, _)| *field)
            },
        );

    let slug_ident = named_ident(slug_field, "SelectorProjection needs named fields")?;
    let slug_ty = &slug_field.ty;
    generics.make_where_clause().predicates.push(parse_quote!(
        #slug_ty: ::std::clone::Clone + ::std::convert::Into<::std::string::String>
    ));
    let title_tokens = if let Some(field) = title_field {
        let title_ident = named_ident(field, "SelectorProjection needs named fields")?;
        let title_ty = &field.ty;
        generics.make_where_clause().predicates.push(parse_quote!(
            #title_ty: ::std::clone::Clone + ::std::convert::Into<::std::string::String>
        ));
        quote! {
            title: ::std::option::Option::Some(self.#title_ident.clone().into())
        }
    } else {
        quote! { title: ::std::option::Option::None }
    };
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #libmcp::SelectorProjection for #ident #ty_generics #where_clause {
            fn selector_ref(&self) -> #libmcp::SelectorRef {
                #libmcp::SelectorRef {
                    slug: self.#slug_ident.clone().into(),
                    #title_tokens,
                }
            }
        }
    })
}

fn named_fields(data: Data, ident: &Ident, derive: &str) -> Result<Vec<Field>> {
    let data = match data {
        Data::Struct(data) => data,
        Data::Enum(_) | Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                format!("{derive} only supports structs"),
            ));
        }
    };
    match data.fields {
        Fields::Named(fields) => Ok(fields.named.into_iter().collect()),
        Fields::Unnamed(_) | Fields::Unit => Err(syn::Error::new_spanned(
            ident,
            format!("{derive} requires named fields"),
        )),
    }
}

fn project_field(
    field: &Field,
    attrs: &FieldAttrs,
    libmcp: &proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    let ident = named_ident(field, "ToolProjection requires named fields")?;
    let key = LitStr::new(field_ident(field).as_str(), ident.span());
    if attrs.has(FieldFlag::SkipNone) {
        Ok(quote! {
            if let ::std::option::Option::Some(value) = &self.#ident {
                object.insert(
                    #key.to_owned(),
                    #libmcp::__macro::serde_json::to_value(value)
                        .map_err(#libmcp::ProjectionError::from)?,
                );
            }
        })
    } else {
        Ok(quote! {
            object.insert(
                #key.to_owned(),
                #libmcp::__macro::serde_json::to_value(&self.#ident)
                    .map_err(#libmcp::ProjectionError::from)?,
            );
        })
    }
}

fn named_ident<'a>(field: &'a Field, message: &str) -> Result<&'a Ident> {
    field
        .ident
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(field, message))
}

fn field_ident(field: &Field) -> String {
    field
        .ident
        .as_ref()
        .map_or_else(String::new, |ident| ident.unraw().to_string())
}

fn unique_marked_field<'a>(
    fields: &'a [(&'a Field, FieldAttrs)],
    marked: impl Fn(&FieldAttrs) -> bool,
    marker: &str,
) -> Result<Option<&'a Field>> {
    let mut matches = fields
        .iter()
        .filter(|(_, attrs)| marked(attrs))
        .map(|(field, _)| *field);
    let first = matches.next();
    if let Some(duplicate) = matches.next() {
        return Err(syn::Error::new_spanned(
            duplicate,
            format!("only one field may be marked {marker}"),
        ));
    }
    Ok(first)
}

#[derive(Default)]
struct ContainerAttrs {
    kind: SurfaceKindAttr,
    kind_seen: bool,
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
                if parsed.kind_seen {
                    return Err(meta.error("duplicate libmcp kind"));
                }
                let kind = meta.value()?.parse::<LitStr>()?;
                parsed.kind = SurfaceKindAttr::parse(&kind)?;
                parsed.kind_seen = true;
                return Ok(());
            }
            if meta.path.is_ident(ATTR_REFERENCE_ONLY) {
                return set_flag(&mut parsed.reference_only, &meta);
            }
            if meta.path.is_ident(ATTR_ALLOW_OPAQUE_IDS) {
                return set_flag(&mut parsed.allow_opaque_ids, &meta);
            }
            Err(meta.error("unsupported libmcp container attribute"))
        })?;
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FieldFlag {
    Selector,
    Title,
    Skip,
    SkipNone,
    FullOnly,
    ConciseOnly,
}

#[derive(Default)]
struct FieldAttrs {
    flags: BTreeSet<FieldFlag>,
}

impl FieldAttrs {
    fn has(&self, flag: FieldFlag) -> bool {
        self.flags.contains(&flag)
    }

    fn projected(&self) -> bool {
        !self.has(FieldFlag::Skip)
    }

    fn in_concise(&self) -> bool {
        self.projected() && !self.has(FieldFlag::FullOnly)
    }

    fn in_full(&self) -> bool {
        self.projected() && !self.has(FieldFlag::ConciseOnly)
    }
}

fn parse_field_attrs(field: &Field) -> Result<FieldAttrs> {
    let mut parsed = FieldAttrs::default();
    for attr in field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident(LIBMCP_ATTR))
    {
        attr.parse_nested_meta(|meta| {
            let flag = if meta.path.is_ident(FIELD_SELECTOR) {
                FieldFlag::Selector
            } else if meta.path.is_ident(FIELD_TITLE) {
                FieldFlag::Title
            } else if meta.path.is_ident(FIELD_SKIP) {
                FieldFlag::Skip
            } else if meta.path.is_ident(FIELD_SKIP_NONE) {
                FieldFlag::SkipNone
            } else if meta.path.is_ident(FIELD_FULL_ONLY) || meta.path.is_ident(FIELD_FULL) {
                FieldFlag::FullOnly
            } else if meta.path.is_ident(FIELD_CONCISE_ONLY) {
                FieldFlag::ConciseOnly
            } else {
                return Err(meta.error("unsupported libmcp field attribute"));
            };
            insert_field_flag(&mut parsed.flags, flag, &meta)
        })?;
    }

    if parsed.has(FieldFlag::Skip)
        && [
            FieldFlag::SkipNone,
            FieldFlag::FullOnly,
            FieldFlag::ConciseOnly,
        ]
        .into_iter()
        .any(|flag| parsed.has(flag))
    {
        return Err(syn::Error::new_spanned(
            field,
            "skip cannot be combined with projection detail attributes",
        ));
    }
    if parsed.has(FieldFlag::FullOnly) && parsed.has(FieldFlag::ConciseOnly) {
        return Err(syn::Error::new_spanned(
            field,
            "full_only and concise_only are mutually exclusive",
        ));
    }
    if parsed.has(FieldFlag::SkipNone) && !is_option(&field.ty) {
        return Err(syn::Error::new_spanned(
            &field.ty,
            "skip_none requires an Option field",
        ));
    }
    if parsed.has(FieldFlag::Selector) && parsed.has(FieldFlag::Title) {
        return Err(syn::Error::new_spanned(
            field,
            "selector and title must identify different fields",
        ));
    }
    Ok(parsed)
}

fn insert_field_flag(
    flags: &mut BTreeSet<FieldFlag>,
    flag: FieldFlag,
    meta: &ParseNestedMeta<'_>,
) -> Result<()> {
    if meta.input.peek(syn::Token![=]) || meta.input.peek(syn::token::Paren) {
        return Err(meta.error("libmcp flag does not take a value"));
    }
    if !flags.insert(flag) {
        return Err(meta.error("duplicate libmcp flag"));
    }
    Ok(())
}

fn set_flag(slot: &mut bool, meta: &ParseNestedMeta<'_>) -> Result<()> {
    if meta.input.peek(syn::Token![=]) || meta.input.peek(syn::token::Paren) {
        return Err(meta.error("libmcp flag does not take a value"));
    }
    if std::mem::replace(slot, true) {
        return Err(meta.error("duplicate libmcp flag"));
    }
    Ok(())
}

fn is_option(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(path) if path.qself.is_none()
            && path.path.segments.last().is_some_and(|segment| segment.ident == "Option")
    )
}

#[derive(Default)]
enum SurfaceKindAttr {
    Overview,
    List,
    #[default]
    Read,
    Mutation,
    Ops,
}

impl SurfaceKindAttr {
    fn parse(kind: &LitStr) -> Result<Self> {
        match kind.value().trim().to_ascii_lowercase().as_str() {
            "overview" => Ok(Self::Overview),
            "list" => Ok(Self::List),
            "read" => Ok(Self::Read),
            "mutation" => Ok(Self::Mutation),
            "ops" => Ok(Self::Ops),
            _ => Err(syn::Error::new_spanned(
                kind,
                "kind must be overview, list, read, mutation, or ops",
            )),
        }
    }

    fn tokens(&self, libmcp: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let variant = match self {
            Self::Overview => quote!(Overview),
            Self::List => quote!(List),
            Self::Read => quote!(Read),
            Self::Mutation => quote!(Mutation),
            Self::Ops => quote!(Ops),
        };
        quote!(#libmcp::SurfaceKind::#variant)
    }
}

fn libmcp_path() -> Result<proc_macro2::TokenStream> {
    match crate_name("libmcp") {
        Ok(FoundCrate::Itself) => Ok(quote!(crate)),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{name}");
            Ok(quote!(::#ident))
        }
        Err(error) => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("failed to locate libmcp crate: {error}"),
        )),
    }
}
