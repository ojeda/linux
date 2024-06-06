// SPDX-License-Identifier: GPL-2.0

//! Clock abstractions.
//!
//! C header: [`include/linux/clk.h`](srctree/include/linux/clk.h)

use crate::{
    bindings,
    device::Device,
    error::{from_err_ptr, Result},
    prelude::*,
};

use core::ptr;

/// A simple implementation of `struct clk` from the C code.
#[repr(transparent)]
pub struct Clk(*mut bindings::clk);

impl Clk {
    /// Creates `Clk` instance for a device and a connection id.
    pub fn new(dev: &Device, name: Option<&CStr>) -> Result<Self> {
        let con_id = if let Some(name) = name {
            name.as_ptr() as *const _
        } else {
            ptr::null()
        };

        // SAFETY: It is safe to call `clk_get()`, on a device pointer earlier received from the C
        // code.
        Ok(Self(from_err_ptr(unsafe {
            bindings::clk_get(dev.as_raw(), con_id)
        })?))
    }

    /// Obtain the raw `struct clk *`.
    pub fn as_raw(&self) -> *mut bindings::clk {
        self.0
    }
}

impl Drop for Clk {
    fn drop(&mut self) {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // relinquish it now.
        unsafe { bindings::clk_put(self.0) };
    }
}
