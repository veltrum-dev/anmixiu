#![forbid(unsafe_code)]

//! Optional derive macros for Anmixiu's capability-oriented element API.

use proc_macro::TokenStream;

/// Derives the public `anmixiu::Element` marker and explicitly requested capabilities.
///
/// Mark one field with `#[element(style)]` to delegate `anmixiu::Styled` to that field. Add
/// `parent` to the same marker when the field also implements `anmixiu::ParentElement`:
///
/// ```ignore
/// #[derive(Element)]
/// struct Card {
///     #[element(style, parent)]
///     root: anmixiu::DivElement,
/// }
/// ```
///
/// The derive does not implement `anmixiu::Lifecycle`; the type must still provide its explicit
/// `render` method and may override lifecycle hooks as needed. It also never infers interaction
/// capabilities from field types.
#[proc_macro_derive(Element, attributes(element))]
pub fn derive_element(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match expand_element(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_element(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let fields = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            syn::Fields::Named(fields) => fields.named.iter().collect::<Vec<_>>(),
            syn::Fields::Unnamed(_) | syn::Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "Element derive requires a struct with named fields",
                ));
            }
        },
        syn::Data::Enum(_) | syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "Element derive requires a struct with named fields",
            ));
        }
    };

    let mut style_field = None;
    let mut parent_field = None;
    for field in fields {
        let Some(ident) = &field.ident else {
            continue;
        };
        collect_markers(field, ident, &mut style_field, &mut parent_field)?;
    }

    let Some((style_ident, style_type)) = style_field else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "Element derive requires one field marked `#[element(style)]`",
        ));
    };
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let (style, style_ref) = if is_style_type(style_type) {
        (
            quote::quote! { &mut self.#style_ident },
            quote::quote! { &self.#style_ident },
        )
    } else {
        (
            quote::quote! { ::anmixiu::Styled::style(&mut self.#style_ident) },
            quote::quote! { ::anmixiu::Styled::style_ref(&self.#style_ident) },
        )
    };

    let parent_impl = parent_field.map(|parent_ident| {
        quote::quote! {
            impl #impl_generics ::anmixiu::ParentElement for #name #ty_generics #where_clause {
                fn child_nodes(&mut self) -> &mut ::std::vec::Vec<::anmixiu::ElementNode> {
                    ::anmixiu::ParentElement::child_nodes(&mut self.#parent_ident)
                }

                fn children_ref(&self) -> &[::anmixiu::ElementNode] {
                    ::anmixiu::ParentElement::children_ref(&self.#parent_ident)
                }
            }
        }
    });

    Ok(quote::quote! {
        impl #impl_generics ::anmixiu::Styled for #name #ty_generics #where_clause {
            fn style(&mut self) -> &mut ::anmixiu::Style {
                #style
            }

            fn style_ref(&self) -> &::anmixiu::Style {
                #style_ref
            }
        }

        #parent_impl

        impl #impl_generics ::anmixiu::Element for #name #ty_generics #where_clause {}
    })
}

fn collect_markers<'a>(
    field: &'a syn::Field,
    ident: &'a syn::Ident,
    style_field: &mut Option<(&'a syn::Ident, &'a syn::Type)>,
    parent_field: &mut Option<&'a syn::Ident>,
) -> syn::Result<()> {
    for attribute in &field.attrs {
        if !attribute.path().is_ident("element") {
            continue;
        }
        let markers = attribute.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        for marker in markers {
            let syn::Meta::Path(path) = marker else {
                return Err(syn::Error::new_spanned(
                    marker,
                    "element attributes must be `style` or `parent`",
                ));
            };
            if path.is_ident("style") {
                if style_field.replace((ident, &field.ty)).is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "Element derive accepts at most one `#[element(style)]` field",
                    ));
                }
            } else if path.is_ident("parent") {
                if parent_field.replace(ident).is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "Element derive accepts at most one `#[element(parent)]` field",
                    ));
                }
            } else {
                return Err(syn::Error::new_spanned(
                    path,
                    "unknown element marker; expected `style` or `parent`",
                ));
            }
        }
    }
    Ok(())
}

fn is_style_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path.segments.last().is_some_and(|segment| {
        segment.ident == "Style" && matches!(segment.arguments, syn::PathArguments::None)
    })
}

#[cfg(test)]
mod tests {
    use super::expand_element;

    #[test]
    fn style_and_parent_markers_generate_both_delegates() {
        let input = syn::parse_str(
            r"
                struct Card {
                    #[element(style, parent)]
                    root: DivElement,
                }
            ",
        )
        .expect("valid derive input");
        let output = expand_element(&input)
            .expect("markers are accepted")
            .to_string();
        assert!(output.contains("ParentElement"));
        assert!(output.contains("Styled"));
        assert!(output.contains("Element"));
    }

    #[test]
    fn missing_style_marker_is_a_targeted_error() {
        let input = syn::parse_str("struct Leaf { value: u32 }").expect("valid derive input");
        let error = expand_element(&input).expect_err("style storage is required");
        assert!(error.to_string().contains("#[element(style)]"));
    }

    #[test]
    fn duplicate_parent_markers_are_rejected() {
        let input = syn::parse_str(
            r"
                struct Invalid {
                    #[element(style, parent)]
                    first: DivElement,
                    #[element(parent)]
                    second: DivElement,
                }
            ",
        )
        .expect("valid derive input");
        let error = expand_element(&input).expect_err("one parent delegate is enough");
        assert!(error.to_string().contains("at most one"));
    }
}
