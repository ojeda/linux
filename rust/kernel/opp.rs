// SPDX-License-Identifier: GPL-2.0

//! Operating performance points.
//!
//! This module provides bindings for interacting with the OPP subsystem.
//!
//! C header: [`include/linux/pm_opp.h`](srctree/include/linux/pm_opp.h)

use crate::{
    bindings,
    device::Device,
    error::{code::*, to_result, Result},
    types::{ARef, AlwaysRefCounted, Opaque},
};

use core::ptr;

/// Dynamically created Operating performance point (OPP).
pub struct Token {
    dev: ARef<Device>,
    freq: usize,
}

impl Token {
    /// Adds an OPP dynamically.
    pub fn new(dev: &ARef<Device>, mut data: Data) -> Result<Self> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_add_dynamic(dev.as_raw(), &mut data.0) })?;
        Ok(Self {
            dev: dev.clone(),
            freq: data.freq(),
        })
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        unsafe { bindings::dev_pm_opp_remove(self.dev.as_raw(), self.freq) };
    }
}

/// Equivalent to `struct dev_pm_opp_data` in the C Code.
#[repr(transparent)]
pub struct Data(bindings::dev_pm_opp_data);

impl Data {
    /// Creates new instance of [`Data`].
    pub fn new(freq: usize, u_volt: usize, level: u32, turbo: bool) -> Self {
        Self(bindings::dev_pm_opp_data {
            turbo,
            freq,
            u_volt,
            level,
        })
    }

    /// Adds an OPP dynamically. The OPP is freed once the [`Token`] gets freed.
    pub fn add_opp(self, dev: &ARef<Device>) -> Result<Token> {
        Token::new(dev, self)
    }

    fn freq(&self) -> usize {
        self.0.freq
    }
}

/// Operating performance point (OPP).
///
/// Wraps the kernel's `struct dev_pm_opp`.
///
/// The pointer to `struct dev_pm_opp` is non-null and valid for the lifetime of the `OPP`
/// instance.
///
/// # Invariants
///
/// Instances of this type are reference-counted. The reference count is incremented by the
/// `dev_pm_opp_get()` function and decremented by `dev_pm_opp_put`. The Rust type `ARef<OPP>`
/// represents a pointer that owns a reference count on the OPP.
///
/// A reference to the `OPP`, `&OPP` isn't refcounted by the Rust code.

#[repr(transparent)]
pub struct OPP(Opaque<bindings::dev_pm_opp>);

// SAFETY: It's OK to send the ownership of `OPP` across thread boundaries.
unsafe impl Send for OPP {}

// SAFETY: It's OK to access `OPP` through shared references from other threads because we're
// either accessing properties that don't change or that are properly synchronised by C code.
unsafe impl Sync for OPP {}

// SAFETY: The type invariants guarantee that [`OPP`] is always refcounted.
unsafe impl AlwaysRefCounted for OPP {
    fn inc_ref(&self) {
        // SAFETY: The existence of a shared reference means that the refcount is nonzero.
        unsafe { bindings::dev_pm_opp_get(self.0.get()) };
    }

    unsafe fn dec_ref(obj: ptr::NonNull<Self>) {
        // SAFETY: The safety requirements guarantee that the refcount is nonzero.
        unsafe { bindings::dev_pm_opp_put(obj.cast().as_ptr()) }
    }
}

impl OPP {
    /// Creates an owned reference to a [`OPP`] from a valid pointer.
    ///
    /// The refcount is incremented by the C code and will be decremented by `dec_ref()` when the
    /// ARef object is dropped.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is valid and the OPP's refcount is incremented. The
    /// caller must also ensure that it doesn't explicitly drop the refcount of the OPP, as the
    /// returned ARef object takes over the refcount increment on the underlying object and the
    /// same will be dropped along with it.
    pub unsafe fn from_raw_opp_owned(ptr: *mut bindings::dev_pm_opp) -> Result<ARef<Self>> {
        let ptr = ptr::NonNull::new(ptr).ok_or(ENODEV)?;

        // SAFETY: The safety requirements guarantee the validity of the pointer.
        Ok(unsafe { ARef::from_raw(ptr.cast()) })
    }

    /// Creates a reference to a [`OPP`] from a valid pointer.
    ///
    /// The refcount is not updated by the Rust API unless the returned reference is converted to
    /// an ARef object.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is valid and remains valid for the duration of 'a.
    pub unsafe fn from_raw_opp<'a>(ptr: *mut bindings::dev_pm_opp) -> Result<&'a Self> {
        // SAFETY: The caller guarantees that the pointer is not dangling and stays valid for the
        // duration of 'a. The cast is okay because `OPP` is `repr(transparent)`.
        Ok(unsafe { &*ptr.cast() })
    }

    #[inline]
    fn as_raw(&self) -> *mut bindings::dev_pm_opp {
        self.0.get()
    }

    /// Returns the frequency of an OPP.
    pub fn freq(&self, index: Option<u32>) -> usize {
        let index = index.unwrap_or(0);

        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_get_freq_indexed(self.as_raw(), index) }
    }

    /// Returns the voltage of an OPP.
    pub fn voltage(&self) -> usize {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_get_voltage(self.as_raw()) }
    }

    /// Returns the level of an OPP.
    pub fn level(&self) -> u32 {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_get_level(self.as_raw()) }
    }

    /// Returns the power of an OPP.
    pub fn power(&self) -> usize {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_get_power(self.as_raw()) }
    }

    /// Returns the required pstate of an OPP.
    pub fn required_pstate(&self, index: u32) -> u32 {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_get_required_pstate(self.as_raw(), index) }
    }

    /// Returns true if the OPP is turbo.
    pub fn is_turbo(&self) -> bool {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it.
        unsafe { bindings::dev_pm_opp_is_turbo(self.as_raw()) }
    }
}
