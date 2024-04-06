// SPDX-License-Identifier: Apache-2.0 OR MIT

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parse_quote, Data, DataStruct, DeriveInput, Error, GenericParam, Result, TypeParam,
    WherePredicate,
};

pub(crate) fn derive(
    DeriveInput {
        ident,
        mut generics,
        data,
        ..
    }: DeriveInput,
    raw_input: TokenStream,
) -> Result<TokenStream> {
    let zeroable_bounds = generics
        .params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Type(t) => Some(t),
            _ => None,
        })
        .map(|TypeParam { ident, .. }| {
            parse_quote! { #ident: ::kernel::init::Zeroable, }
        })
        .collect::<Vec<WherePredicate>>();
    generics
        .make_where_clause()
        .predicates
        .extend(zeroable_bounds);
    let (impl_g, type_g, whr) = generics.split_for_impl();
    let Data::Struct(DataStruct { fields, .. }) = data else {
        return Err(Error::new_spanned(
            raw_input,
            "`Zeroable` can only be derived for structs.",
        ));
    };
    let field_ty = fields.iter().map(|f| &f.ty);
    Ok(quote! {
        // SAFETY: Every field type implements `Zeroable` and padding bytes may be zero.
        #[automatically_derived]
        unsafe impl #impl_g ::kernel::init::Zeroable for #ident #type_g
            #whr
        {}
        const _: () = {
            fn assert_zeroable<T: ?::core::marker::Sized + ::kernel::init::Zeroable>() {}
            fn ensure_zeroable #impl_g ()
                #whr
            {
                #(assert_zeroable::<#field_ty>();)*
            }
        };
    })
}
