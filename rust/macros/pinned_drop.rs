// SPDX-License-Identifier: Apache-2.0 OR MIT

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_quote, Error, ImplItem, ImplItemFn, ItemImpl, Token};

struct PinnedDropImpl(ItemImpl, ImplItemFn);

impl TryFrom<ItemImpl> for PinnedDropImpl {
    type Error = (Error, TokenStream);
    fn try_from(mut impl_: ItemImpl) -> Result<Self, Self::Error> {
        let aux_impl = match &impl_ {
            ItemImpl {
                attrs,
                impl_token,
                generics,
                trait_: Some((polarity, path, for_)),
                self_ty,
                ..
            } if path.is_ident("PinnedDrop") => {
                let (impl_generics, _, whr) = generics.split_for_impl();
                quote! {
                    #(#attrs)*
                    unsafe #impl_token #impl_generics #polarity ::kernel::init::PinnedDrop #for_ #self_ty
                        #whr
                    {
                        fn drop(
                            self: ::core::pin::Pin<&mut Self>,
                            _: ::kernel::init::__internal::OnlyCallFromDrop,
                        ) {}
                    }
                }
            }
            _ => quote!(#impl_),
        };
        if impl_.unsafety.is_some() {
            return Err((
                Error::new_spanned(
                    impl_.unsafety,
                    "The `PinnedDrop` impl must not be `unsafe`.",
                ),
                aux_impl,
            ));
        }
        let trait_tokens = match &impl_.trait_ {
            None => quote!(),
            Some((None, path, _)) => quote!(#path),
            Some((Some(not), path, _)) => quote!(#not #path),
        };
        match &impl_.trait_ {
            Some((None, path, _)) if path.is_ident("PinnedDrop") => {}
            None | Some((None, ..)) => {
                return Err((
                    Error::new_spanned(
                        trait_tokens,
                        "`#[pinned_drop]` can only be used on `PinnedDrop` impls.",
                    ),
                    aux_impl,
                ));
            }
            Some((Some(_), ..)) => {
                return Err((
                    Error::new_spanned(
                        trait_tokens,
                        "`#[pinned_drop]` can only be used on positive `PinnedDrop` impls.",
                    ),
                    aux_impl,
                ));
            }
        }
        if impl_.items.len() != 1 {
            return Err((
                Error::new(
                    impl_.brace_token.span.join(),
                    "Expected exactly one function in the `PinnedDrop` impl.",
                ),
                aux_impl,
            ));
        }
        let ImplItem::Fn(drop) = impl_.items.pop().unwrap() else {
            return Err((
                Error::new(impl_.brace_token.span.join(), "Expected a function."),
                aux_impl,
            ));
        };

        Ok(Self(impl_, drop))
    }
}

impl ToTokens for PinnedDropImpl {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.0.to_tokens(tokens)
    }
}

pub(crate) fn pinned_drop(drop_impl: ItemImpl) -> Result<TokenStream, (Error, TokenStream)> {
    let PinnedDropImpl(mut drop_impl, mut drop) = drop_impl.try_into()?;
    drop.sig
        .inputs
        .push(parse_quote!(_: ::kernel::init::__internal::OnlyCallFromDrop));
    let span = drop_impl.impl_token.span;

    drop_impl.items.push(ImplItem::Fn(drop));
    drop_impl.unsafety = Some(Token![unsafe](span));
    drop_impl.trait_ = Some((
        None,
        parse_quote!(::kernel::init::PinnedDrop),
        Token![for](span),
    ));
    Ok(quote! { #drop_impl })
}
