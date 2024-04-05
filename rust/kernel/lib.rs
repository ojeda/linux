// SPDX-License-Identifier: GPL-2.0

//! The `kernel` crate.
//!
//! This crate contains the kernel APIs that have been ported or wrapped for
//! usage by Rust code in the kernel and is shared by all of them.
//!
//! In other words, all the rest of the Rust code in the kernel (e.g. kernel
//! modules written in Rust) depends on [`core`], [`alloc`] and this crate.
//!
//! If you need a kernel C API that is not ported or wrapped yet here, then
//! do so first instead of bypassing this crate.

#![no_std]
#![feature(allocator_api)]
#![feature(coerce_unsized)]
#![feature(dispatch_from_dyn)]
#![feature(new_uninit)]
#![feature(receiver_trait)]
#![feature(unsize)]

// Ensure conditional compilation based on the kernel configuration works;
// otherwise we may silently break things like initcall handling.
#[cfg(not(CONFIG_RUST))]
compile_error!("Missing kernel configuration for conditional compilation");

// Allow proc-macros to refer to `::kernel` inside the `kernel` crate (this crate).
extern crate self as kernel;

#[cfg(not(test))]
#[cfg(not(testlib))]
mod allocator;
mod build_assert;
pub mod error;
pub mod init;
pub mod ioctl;
#[cfg(CONFIG_KUNIT)]
pub mod kunit;
#[cfg(CONFIG_NET)]
pub mod net;
pub mod prelude;
pub mod print;
mod static_assert;
#[doc(hidden)]
pub mod std_vendor;
pub mod str;
pub mod sync;
pub mod task;
pub mod time;
pub mod types;
pub mod workqueue;

#[doc(hidden)]
pub use bindings;
pub use macros;
pub use uapi;

#[doc(hidden)]
pub use build_error::build_error;

/// Prefix to appear before log messages printed from within the `kernel` crate.
const __LOG_PREFIX: &[u8] = b"rust_kernel\0";

/// The top level entrypoint to implementing a kernel module.
///
/// For any teardown or cleanup operations, your type may implement [`Drop`].
pub trait Module: Sized + Sync {
    /// Called at module initialization time.
    ///
    /// Use this method to perform whatever setup or registration your module
    /// should do.
    ///
    /// Equivalent to the `module_init` macro in the C API.
    fn init(module: &'static ThisModule) -> error::Result<Self>;
}

/// Equivalent to `THIS_MODULE` in the C API.
///
/// C header: [`include/linux/export.h`](srctree/include/linux/export.h)
pub struct ThisModule(*mut bindings::module);

// SAFETY: `THIS_MODULE` may be used from all threads within a module.
unsafe impl Sync for ThisModule {}

impl ThisModule {
    /// Creates a [`ThisModule`] given the `THIS_MODULE` pointer.
    ///
    /// # Safety
    ///
    /// The pointer must be equal to the right `THIS_MODULE`.
    pub const unsafe fn from_ptr(ptr: *mut bindings::module) -> ThisModule {
        ThisModule(ptr)
    }

    /// Access the raw pointer for this module.
    ///
    /// It is up to the user to use it correctly.
    pub const fn as_ptr(&self) -> *mut bindings::module {
        self.0
    }
}

#[cfg(not(any(testlib, test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    pr_emerg!("{}\n", info);
    // SAFETY: FFI call.
    unsafe { bindings::BUG() };
}

/// Produces a pointer to an object from a pointer to one of its fields.
///
/// # Safety
///
/// The pointer passed to this macro, and the pointer returned by this macro, must both be in
/// bounds of the same allocation.
///
/// # Examples
///
/// ```
/// # use kernel::container_of;
/// struct Test {
///     a: u64,
///     b: u32,
/// }
///
/// let test = Test { a: 10, b: 20 };
/// let b_ptr = &test.b;
/// // SAFETY: The pointer points at the `b` field of a `Test`, so the resulting pointer will be
/// // in-bounds of the same allocation as `b_ptr`.
/// let test_alias = unsafe { container_of!(b_ptr, Test, b) };
/// assert!(core::ptr::eq(&test, test_alias));
/// ```
#[macro_export]
macro_rules! container_of {
    ($ptr:expr, $type:ty, $($f:tt)*) => {{
        let ptr = $ptr as *const _ as *const u8;
        let offset: usize = ::core::mem::offset_of!($type, $($f)*);
        ptr.sub(offset) as *const $type
    }}
}

mod heapsize {

    pub(crate) trait HeapSize {
        /// Total number of bytes of heap memory owned by `self`.
        ///
        /// Does not include the size of `self` itself, which may or may not be on
        /// the heap. Includes only children of `self`, meaning things pointed to by
        /// `self`.
        fn heap_size_of_children(&self) -> usize;
    }

    //
    // In a real version of this library there would be lots more impls here, but
    // here are some interesting ones.
    //

    impl HeapSize for u8 {
        /// A `u8` does not own any heap memory.
        fn heap_size_of_children(&self) -> usize {
            0
        }
    }

    impl<T> HeapSize for alloc::boxed::Box<T>
    where
        T: ?Sized + HeapSize,
    {
        /// A `Box` owns however much heap memory was allocated to hold the value of
        /// type `T` that we placed on the heap, plus transitively however much `T`
        /// itself owns.
        fn heap_size_of_children(&self) -> usize {
            core::mem::size_of_val(&**self) + (**self).heap_size_of_children()
        }
    }

    impl<T> HeapSize for [T]
    where
        T: HeapSize,
    {
        /// Sum of heap memory owned by each element of a dynamically sized slice of
        /// `T`.
        fn heap_size_of_children(&self) -> usize {
            self.iter().map(HeapSize::heap_size_of_children).sum()
        }
    }

    impl<'a, T> HeapSize for &'a T
    where
        T: ?Sized,
    {
        /// A shared reference does not own heap memory.
        fn heap_size_of_children(&self) -> usize {
            0
        }
    }
}

use macros::HeapSize;

#[derive(HeapSize)]
#[allow(clippy::redundant_allocation)]
struct Test {
    a: u8,
    b: alloc::boxed::Box<alloc::boxed::Box<u8>>,
}

/// Test `syn`.
pub fn testsyn() {
    use crate::heapsize::HeapSize;
    let b = alloc::boxed::Box::try_new(42).unwrap();
    let test = Test {
        a: 42,
        b: alloc::boxed::Box::try_new(b).unwrap(),
    };
    pr_info!("{}\n", test.heap_size_of_children());
}
