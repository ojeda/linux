// SPDX-License-Identifier: Apache-2.0 OR MIT

//! This module provides the macros that actually implement the proc-macros `pin_data` and
//! `pinned_drop`. It also contains `__init_internal` the implementation of the `{try_}{pin_}init!`
//! macros.
//!
//! These macros should never be called directly, since they expect their input to be
//! in a certain format which is internal. If used incorrectly, these macros can lead to UB even in
//! safe code! Use the public facing macros instead.
//!
//! This architecture has been chosen because the kernel does not yet have access to `syn` which
//! would make matters a lot easier for implementing these as proc-macros.
//!
//! # Macro expansion example
//!
//! This section is intended for readers trying to understand the macros in this module and the
//! `pin_init!` macros from `init.rs`.
//!
//! We will look at the following example:
//!
//! ```rust,ignore
//! # use kernel::init::*;
//! # use core::pin::Pin;
//! #[pin_data]
//! #[repr(C)]
//! struct Bar<T> {
//!     #[pin]
//!     t: T,
//!     pub x: usize,
//! }
//!
//! impl<T> Bar<T> {
//!     fn new(t: T) -> impl PinInit<Self> {
//!         pin_init!(Self { t, x: 0 })
//!     }
//! }
//!
//! #[pin_data(PinnedDrop)]
//! struct Foo {
//!     a: usize,
//!     #[pin]
//!     b: Bar<u32>,
//! }
//!
//! #[pinned_drop]
//! impl PinnedDrop for Foo {
//!     fn drop(self: Pin<&mut Self>) {
//!         pr_info!("{self:p} is getting dropped.");
//!     }
//! }
//!
//! let a = 42;
//! let initializer = pin_init!(Foo {
//!     a,
//!     b <- Bar::new(36),
//! });
//! ```
//!
//! This example includes the most common and important features of the pin-init API.
//!
//! Below you can find individual section about the different macro invocations. Here are some
//! general things we need to take into account when designing macros:
//! - use global paths, similarly to file paths, these start with the separator: `::core::panic!()`
//!   this ensures that the correct item is used, since users could define their own `mod core {}`
//!   and then their own `panic!` inside to execute arbitrary code inside of our macro.
//! - macro `unsafe` hygiene: we need to ensure that we do not expand arbitrary, user-supplied
//!   expressions inside of an `unsafe` block in the macro, because this would allow users to do
//!   `unsafe` operations without an associated `unsafe` block.
//!
//! ## `#[pin_data]` on `Bar`
//!
//! This macro is used to specify which fields are structurally pinned and which fields are not. It
//! is placed on the struct definition and allows `#[pin]` to be placed on the fields.
//!
//! Here is the definition of `Bar` from our example:
//!
//! ```rust,ignore
//! # use kernel::init::*;
//! #[pin_data]
//! #[repr(C)]
//! struct Bar<T> {
//!     #[pin]
//!     t: T,
//!     pub x: usize,
//! }
//! ```
//!
//! This expands to the following code:
//!
//! ```rust,ignore
//! // Firstly the normal definition of the struct, attributes are preserved:
//! #[repr(C)]
//! struct Bar<T> {
//!     t: T,
//!     pub x: usize,
//! }
//! // Then an anonymous constant is defined, this is because we do not want any code to access the
//! // types that we define inside:
//! const _: () = {
//!     // We define the pin-data carrying struct, it is a ZST and needs to have the same generics,
//!     // since we need to implement access functions for each field and thus need to know its
//!     // type.
//!     struct __ThePinData<T> {
//!         __phantom: ::core::marker::PhantomData<fn(Bar<T>) -> Bar<T>>,
//!     }
//!     // We implement `Copy` for the pin-data struct, since all functions it defines will take
//!     // `self` by value.
//!     impl<T> ::core::clone::Clone for __ThePinData<T> {
//!         fn clone(&self) -> Self {
//!             *self
//!         }
//!     }
//!     impl<T> ::core::marker::Copy for __ThePinData<T> {}
//!     // For every field of `Bar`, the pin-data struct will define a function with the same name
//!     // and accessor (`pub` or `pub(crate)` etc.). This function will take a pointer to the
//!     // field (`slot`) and a `PinInit` or `Init` depending on the projection kind of the field
//!     // (if pinning is structural for the field, then `PinInit` otherwise `Init`).
//!     #[allow(dead_code)]
//!     impl<T> __ThePinData<T> {
//!         unsafe fn t<E>(
//!             self,
//!             slot: *mut T,
//!             // Since `t` is `#[pin]`, this is `PinInit`.
//!             init: impl ::kernel::init::PinInit<T, E>,
//!         ) -> ::core::result::Result<(), E> {
//!             unsafe { ::kernel::init::PinInit::__pinned_init(init, slot) }
//!         }
//!         pub unsafe fn x<E>(
//!             self,
//!             slot: *mut usize,
//!             // Since `x` is not `#[pin]`, this is `Init`.
//!             init: impl ::kernel::init::Init<usize, E>,
//!         ) -> ::core::result::Result<(), E> {
//!             unsafe { ::kernel::init::Init::__init(init, slot) }
//!         }
//!     }
//!     // Implement the internal `HasPinData` trait that associates `Bar` with the pin-data struct
//!     // that we constructed above.
//!     unsafe impl<T> ::kernel::init::__internal::HasPinData for Bar<T> {
//!         type PinData = __ThePinData<T>;
//!         unsafe fn __pin_data() -> Self::PinData {
//!             __ThePinData {
//!                 __phantom: ::core::marker::PhantomData,
//!             }
//!         }
//!     }
//!     // Implement the internal `PinData` trait that marks the pin-data struct as a pin-data
//!     // struct. This is important to ensure that no user can implement a rouge `__pin_data`
//!     // function without using `unsafe`.
//!     unsafe impl<T> ::kernel::init::__internal::PinData for __ThePinData<T> {
//!         type Datee = Bar<T>;
//!     }
//!     // Now we only want to implement `Unpin` for `Bar` when every structurally pinned field is
//!     // `Unpin`. In other words, whether `Bar` is `Unpin` only depends on structurally pinned
//!     // fields (those marked with `#[pin]`). These fields will be listed in this struct, in our
//!     // case no such fields exist, hence this is almost empty. The two phantomdata fields exist
//!     // for two reasons:
//!     // - `__phantom`: every generic must be used, since we cannot really know which generics
//!     //   are used, we declere all and then use everything here once.
//!     // - `__phantom_pin`: uses the `'__pin` lifetime and ensures that this struct is invariant
//!     //   over it. The lifetime is needed to work around the limitation that trait bounds must
//!     //   not be trivial, e.g. the user has a `#[pin] PhantomPinned` field -- this is
//!     //   unconditionally `!Unpin` and results in an error. The lifetime tricks the compiler
//!     //   into accepting these bounds regardless.
//!     #[allow(dead_code)]
//!     struct __Unpin<'__pin, T> {
//!         __phantom_pin: ::core::marker::PhantomData<fn(&'__pin ()) -> &'__pin ()>,
//!         __phantom: ::core::marker::PhantomData<fn(Bar<T>) -> Bar<T>>,
//!         // Our only `#[pin]` field is `t`.
//!         t: T,
//!     }
//!     #[doc(hidden)]
//!     impl<'__pin, T> ::core::marker::Unpin for Bar<T>
//!     where
//!         __Unpin<'__pin, T>: ::core::marker::Unpin,
//!     {}
//!     // Now we need to ensure that `Bar` does not implement `Drop`, since that would give users
//!     // access to `&mut self` inside of `drop` even if the struct was pinned. This could lead to
//!     // UB with only safe code, so we disallow this by giving a trait implementation error using
//!     // a direct impl and a blanket implementation.
//!     trait MustNotImplDrop {}
//!     // Normally `Drop` bounds do not have the correct semantics, but for this purpose they do
//!     // (normally people want to know if a type has any kind of drop glue at all, here we want
//!     // to know if it has any kind of custom drop glue, which is exactly what this bound does).
//!     #[allow(drop_bounds)]
//!     impl<T: ::core::ops::Drop> MustNotImplDrop for T {}
//!     impl<T> MustNotImplDrop for Bar<T> {}
//!     // Here comes a convenience check, if one implemented `PinnedDrop`, but forgot to add it to
//!     // `#[pin_data]`, then this will error with the same mechanic as above, this is not needed
//!     // for safety, but a good sanity check, since no normal code calls `PinnedDrop::drop`.
//!     #[allow(non_camel_case_types)]
//!     trait UselessPinnedDropImpl_you_need_to_specify_PinnedDrop {}
//!     impl<
//!         T: ::kernel::init::PinnedDrop,
//!     > UselessPinnedDropImpl_you_need_to_specify_PinnedDrop for T {}
//!     impl<T> UselessPinnedDropImpl_you_need_to_specify_PinnedDrop for Bar<T> {}
//! };
//! ```
//!
//! ## `pin_init!` in `impl Bar`
//!
//! This macro creates an pin-initializer for the given struct. It requires that the struct is
//! annotated by `#[pin_data]`.
//!
//! Here is the impl on `Bar` defining the new function:
//!
//! ```rust,ignore
//! impl<T> Bar<T> {
//!     fn new(t: T) -> impl PinInit<Self> {
//!         pin_init!(Self { t, x: 0 })
//!     }
//! }
//! ```
//!
//! This expands to the following code:
//!
//! ```rust,ignore
//! impl<T> Bar<T> {
//!     fn new(t: T) -> impl PinInit<Self> {
//!         {
//!             // We do not want to allow arbitrary returns, so we declare this type as the `Ok`
//!             // return type and shadow it later when we insert the arbitrary user code. That way
//!             // there will be no possibility of returning without `unsafe`.
//!             struct __InitOk;
//!             // Get the data about fields from the supplied type.
//!             // - the function is unsafe, hence the unsafe block
//!             // - we `use` the `HasPinData` trait in the block, it is only available in that
//!             //   scope.
//!             let data = unsafe {
//!                 use ::kernel::init::__internal::HasPinData;
//!                 Self::__pin_data()
//!             };
//!             // Ensure that `data` really is of type `PinData` and help with type inference:
//!             let init = ::kernel::init::__internal::PinData::make_closure::<
//!                 _,
//!                 __InitOk,
//!                 ::core::convert::Infallible,
//!             >(data, move |slot| {
//!                 {
//!                     // Shadow the structure so it cannot be used to return early. If a user
//!                     // tries to write `return Ok(__InitOk)`, then they get a type error,
//!                     // since that will refer to this struct instead of the one defined
//!                     // above.
//!                     struct __InitOk;
//!                     // This is the expansion of `t,`, which is syntactic sugar for `t: t,`.
//!                     {
//!                         unsafe { ::core::ptr::write(::core::addr_of_mut!((*slot).t), t) };
//!                     }
//!                     // Since initialization could fail later (not in this case, since the
//!                     // error type is `Infallible`) we will need to drop this field if there
//!                     // is an error later. This `DropGuard` will drop the field when it gets
//!                     // dropped and has not yet been forgotten.
//!                     let t = unsafe {
//!                         ::pinned_init::__internal::DropGuard::new(::core::addr_of_mut!((*slot).t))
//!                     };
//!                     // Expansion of `x: 0,`:
//!                     // Since this can be an arbitrary expression we cannot place it inside
//!                     // of the `unsafe` block, so we bind it here.
//!                     {
//!                         let x = 0;
//!                         unsafe { ::core::ptr::write(::core::addr_of_mut!((*slot).x), x) };
//!                     }
//!                     // We again create a `DropGuard`.
//!                     let x = unsafe {
//!                         ::kernel::init::__internal::DropGuard::new(::core::addr_of_mut!((*slot).x))
//!                     };
//!                     // Since initialization has successfully completed, we can now forget
//!                     // the guards. This is not `mem::forget`, since we only have
//!                     // `&DropGuard`.
//!                     ::core::mem::forget(x);
//!                     ::core::mem::forget(t);
//!                     // Here we use the type checker to ensure that every field has been
//!                     // initialized exactly once, since this is `if false` it will never get
//!                     // executed, but still type-checked.
//!                     // Additionally we abuse `slot` to automatically infer the correct type
//!                     // for the struct. This is also another check that every field is
//!                     // accessible from this scope.
//!                     #[allow(unreachable_code, clippy::diverging_sub_expression)]
//!                     let _ = || {
//!                         unsafe {
//!                             ::core::ptr::write(
//!                                 slot,
//!                                 Self {
//!                                     // We only care about typecheck finding every field
//!                                     // here, the expression does not matter, just conjure
//!                                     // one using `panic!()`:
//!                                     t: ::core::panic!(),
//!                                     x: ::core::panic!(),
//!                                 },
//!                             );
//!                         };
//!                     };
//!                 }
//!                 // We leave the scope above and gain access to the previously shadowed
//!                 // `__InitOk` that we need to return.
//!                 Ok(__InitOk)
//!             });
//!             // Change the return type from `__InitOk` to `()`.
//!             let init = move |
//!                 slot,
//!             | -> ::core::result::Result<(), ::core::convert::Infallible> {
//!                 init(slot).map(|__InitOk| ())
//!             };
//!             // Construct the initializer.
//!             let init = unsafe {
//!                 ::kernel::init::pin_init_from_closure::<
//!                     _,
//!                     ::core::convert::Infallible,
//!                 >(init)
//!             };
//!             init
//!         }
//!     }
//! }
//! ```
//!
//! ## `#[pin_data]` on `Foo`
//!
//! Since we already took a look at `#[pin_data]` on `Bar`, this section will only explain the
//! differences/new things in the expansion of the `Foo` definition:
//!
//! ```rust,ignore
//! #[pin_data(PinnedDrop)]
//! struct Foo {
//!     a: usize,
//!     #[pin]
//!     b: Bar<u32>,
//! }
//! ```
//!
//! This expands to the following code:
//!
//! ```rust,ignore
//! struct Foo {
//!     a: usize,
//!     b: Bar<u32>,
//! }
//! const _: () = {
//!     struct __ThePinData {
//!         __phantom: ::core::marker::PhantomData<fn(Foo) -> Foo>,
//!     }
//!     impl ::core::clone::Clone for __ThePinData {
//!         fn clone(&self) -> Self {
//!             *self
//!         }
//!     }
//!     impl ::core::marker::Copy for __ThePinData {}
//!     #[allow(dead_code)]
//!     impl __ThePinData {
//!         unsafe fn b<E>(
//!             self,
//!             slot: *mut Bar<u32>,
//!             init: impl ::kernel::init::PinInit<Bar<u32>, E>,
//!         ) -> ::core::result::Result<(), E> {
//!             unsafe { ::kernel::init::PinInit::__pinned_init(init, slot) }
//!         }
//!         unsafe fn a<E>(
//!             self,
//!             slot: *mut usize,
//!             init: impl ::kernel::init::Init<usize, E>,
//!         ) -> ::core::result::Result<(), E> {
//!             unsafe { ::kernel::init::Init::__init(init, slot) }
//!         }
//!     }
//!     unsafe impl ::kernel::init::__internal::HasPinData for Foo {
//!         type PinData = __ThePinData;
//!         unsafe fn __pin_data() -> Self::PinData {
//!             __ThePinData {
//!                 __phantom: ::core::marker::PhantomData,
//!             }
//!         }
//!     }
//!     unsafe impl ::kernel::init::__internal::PinData for __ThePinData {
//!         type Datee = Foo;
//!     }
//!     #[allow(dead_code)]
//!     struct __Unpin<'__pin> {
//!         __phantom_pin: ::core::marker::PhantomData<fn(&'__pin ()) -> &'__pin ()>,
//!         __phantom: ::core::marker::PhantomData<fn(Foo) -> Foo>,
//!         b: Bar<u32>,
//!     }
//!     #[doc(hidden)]
//!     impl<'__pin> ::core::marker::Unpin for Foo
//!     where
//!         __Unpin<'__pin>: ::core::marker::Unpin,
//!     {}
//!     // Since we specified `PinnedDrop` as the argument to `#[pin_data]`, we expect `Foo` to
//!     // implement `PinnedDrop`. Thus we do not need to prevent `Drop` implementations like
//!     // before, instead we implement `Drop` here and delegate to `PinnedDrop`.
//!     impl ::core::ops::Drop for Foo {
//!         fn drop(&mut self) {
//!             // Since we are getting dropped, no one else has a reference to `self` and thus we
//!             // can assume that we never move.
//!             let pinned = unsafe { ::core::pin::Pin::new_unchecked(self) };
//!             // Create the unsafe token that proves that we are inside of a destructor, this
//!             // type is only allowed to be created in a destructor.
//!             let token = unsafe { ::kernel::init::__internal::OnlyCallFromDrop::new() };
//!             ::kernel::init::PinnedDrop::drop(pinned, token);
//!         }
//!     }
//! };
//! ```
//!
//! ## `#[pinned_drop]` on `impl PinnedDrop for Foo`
//!
//! This macro is used to implement the `PinnedDrop` trait, since that trait is `unsafe` and has an
//! extra parameter that should not be used at all. The macro hides that parameter.
//!
//! Here is the `PinnedDrop` impl for `Foo`:
//!
//! ```rust,ignore
//! #[pinned_drop]
//! impl PinnedDrop for Foo {
//!     fn drop(self: Pin<&mut Self>) {
//!         pr_info!("{self:p} is getting dropped.");
//!     }
//! }
//! ```
//!
//! This expands to the following code:
//!
//! ```rust,ignore
//! // `unsafe`, full path and the token parameter are added, everything else stays the same.
//! unsafe impl ::kernel::init::PinnedDrop for Foo {
//!     fn drop(self: Pin<&mut Self>, _: ::kernel::init::__internal::OnlyCallFromDrop) {
//!         pr_info!("{self:p} is getting dropped.");
//!     }
//! }
//! ```
//!
//! ## `pin_init!` on `Foo`
//!
//! Since we already took a look at `pin_init!` on `Bar`, this section will only show the expansion
//! of `pin_init!` on `Foo`:
//!
//! ```rust,ignore
//! let a = 42;
//! let initializer = pin_init!(Foo {
//!     a,
//!     b <- Bar::new(36),
//! });
//! ```
//!
//! This expands to the following code:
//!
//! ```rust,ignore
//! let a = 42;
//! let initializer = {
//!     struct __InitOk;
//!     let data = unsafe {
//!         use ::kernel::init::__internal::HasPinData;
//!         Foo::__pin_data()
//!     };
//!     let init = ::kernel::init::__internal::PinData::make_closure::<
//!         _,
//!         __InitOk,
//!         ::core::convert::Infallible,
//!     >(data, move |slot| {
//!         {
//!             struct __InitOk;
//!             {
//!                 unsafe { ::core::ptr::write(::core::addr_of_mut!((*slot).a), a) };
//!             }
//!             let a = unsafe {
//!                 ::kernel::init::__internal::DropGuard::new(::core::addr_of_mut!((*slot).a))
//!             };
//!             let init = Bar::new(36);
//!             unsafe { data.b(::core::addr_of_mut!((*slot).b), b)? };
//!             let b = unsafe {
//!                 ::kernel::init::__internal::DropGuard::new(::core::addr_of_mut!((*slot).b))
//!             };
//!             ::core::mem::forget(b);
//!             ::core::mem::forget(a);
//!             #[allow(unreachable_code, clippy::diverging_sub_expression)]
//!             let _ = || {
//!                 unsafe {
//!                     ::core::ptr::write(
//!                         slot,
//!                         Foo {
//!                             a: ::core::panic!(),
//!                             b: ::core::panic!(),
//!                         },
//!                     );
//!                 };
//!             };
//!         }
//!         Ok(__InitOk)
//!     });
//!     let init = move |
//!         slot,
//!     | -> ::core::result::Result<(), ::core::convert::Infallible> {
//!         init(slot).map(|__InitOk| ())
//!     };
//!     let init = unsafe {
//!         ::kernel::init::pin_init_from_closure::<_, ::core::convert::Infallible>(init)
//!     };
//!     init
//! };
//! ```
