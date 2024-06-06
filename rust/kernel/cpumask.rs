// SPDX-License-Identifier: GPL-2.0

//! CPU mask abstractions.
//!
//! C header: [`include/linux/cpumask.h`](srctree/include/linux/cpumask.h)

use crate::{bindings, error::Result, prelude::ENOMEM};
use core::ptr;

/// A simple implementation of `struct cpumask` from the C code.
pub struct Cpumask {
    ptr: *mut bindings::cpumask,
    owned: bool,
}

impl Cpumask {
    /// Creates empty cpumask.
    pub fn new() -> Result<Self> {
        let mut ptr: *mut bindings::cpumask = ptr::null_mut();

        // SAFETY: Depending on the value of `gfp_flags`, this call may sleep. Other than that, it
        // is always safe to call this method.
        if !unsafe { bindings::zalloc_cpumask_var(&mut ptr, bindings::GFP_KERNEL) } {
            return Err(ENOMEM);
        }

        Ok(Self { ptr, owned: true })
    }

    /// Creates a new abstraction instance of an existing `struct cpumask` pointer.
    ///
    /// # Safety
    ///
    /// Callers must ensure that `ptr` is valid, and non-null.
    pub unsafe fn get_cpumask(ptr: *mut bindings::cpumask) -> Self {
        Self { ptr, owned: false }
    }

    /// Obtain the raw `struct cpumask *`.
    pub fn as_raw(&mut self) -> *mut bindings::cpumask {
        self.ptr
    }

    /// Sets CPU in the cpumask.
    ///
    /// Update the cpumask with a single CPU.
    pub fn set(&mut self, cpu: u32) {
        // SAFETY: `ptr` is guaranteed to be valid for the lifetime of `self`. And it is safe to
        // call `cpumask_set_cpus()` for any CPU.
        unsafe { bindings::cpumask_set_cpu(cpu, self.ptr) };
    }

    /// Clears CPU in the cpumask.
    ///
    /// Update the cpumask with a single CPU.
    pub fn clear(&mut self, cpu: i32) {
        // SAFETY: `ptr` is guaranteed to be valid for the lifetime of `self`. And it is safe to
        // call `cpumask_clear_cpu()` for any CPU.
        unsafe { bindings::cpumask_clear_cpu(cpu, self.ptr) };
    }

    /// Sets all CPUs in the cpumask.
    pub fn set_all(&mut self) {
        // SAFETY: `ptr` is guaranteed to be valid for the lifetime of `self`. And it is safe to
        // call `cpumask_setall()`.
        unsafe { bindings::cpumask_setall(self.ptr) };
    }

    /// Copies cpumask.
    pub fn copy(&self, dstp: &mut Self) {
        // SAFETY: `ptr` is guaranteed to be valid for the lifetime of `self`. And it is safe to
        // call `cpumask_copy()`.
        unsafe { bindings::cpumask_copy(dstp.as_raw(), self.ptr) };
    }
}

impl Drop for Cpumask {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: `ptr` is guaranteed to be valid for the lifetime of `self`. And it is safe
            // to call `free_cpumask_var()`.
            unsafe { bindings::free_cpumask_var(self.ptr) }
        }
    }
}
