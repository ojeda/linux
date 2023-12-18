// SPDX-License-Identifier: GPL-2.0

//! Operating performance points.
//!
//! This module provides bindings for interacting with the OPP subsystem.
//!
//! C header: [`include/linux/pm_opp.h`](srctree/include/linux/pm_opp.h)

use crate::{
    bindings,
    cpumask::Cpumask,
    device::Device,
    error::{code::*, from_err_ptr, to_result, Error, Result},
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

/// OPP search types.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SearchType {
    /// Search for exact value.
    Exact,
    /// Search for highest value less than equal to value.
    Floor,
    /// Search for lowest value greater than equal to value.
    Ceil,
}

/// Operating performance point (OPP) table.
///
/// Wraps the kernel's `struct opp_table`.
///
/// The pointer stored in `Self` is non-null and valid for the lifetime of the `Table`.
pub struct Table {
    ptr: *mut bindings::opp_table,
    dev: ARef<Device>,
    em: bool,
    of: bool,
    cpumask: Option<Cpumask>,
}

// SAFETY: It is okay to send ownership of `Table` across thread boundaries.
unsafe impl Send for Table {}

// SAFETY: It's OK to access `Table` through shared references from other threads because we're
// either accessing properties that don't change or that are properly synchronised by C code.
unsafe impl Sync for Table {}

impl Table {
    /// Creates a new OPP table instance from raw pointer.
    ///
    /// # Safety
    ///
    /// Callers must ensure that `ptr` is valid and non-null.
    unsafe fn from_raw_table(ptr: *mut bindings::opp_table, dev: &ARef<Device>) -> Self {
        // SAFETY: By the safety requirements, ptr is valid and its refcount will be incremented.
        unsafe { bindings::dev_pm_opp_get_opp_table_ref(ptr) };

        Self {
            ptr,
            dev: dev.clone(),
            em: false,
            of: false,
            cpumask: None,
        }
    }

    /// Find OPP table from device.
    pub fn from_dev(dev: &Device) -> Result<Self> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements. Refcount of the OPP table is incremented by the C code.
        let ptr = from_err_ptr(unsafe { bindings::dev_pm_opp_get_opp_table(dev.as_raw()) })?;

        Ok(Self {
            ptr,
            dev: dev.into(),
            em: false,
            of: false,
            cpumask: None,
        })
    }

    /// Add device tree based OPP table for the device.
    #[cfg(CONFIG_OF)]
    pub fn from_of(dev: &ARef<Device>, index: i32) -> Result<Self> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements. Refcount of the OPP table is incremented by the C code.
        to_result(unsafe { bindings::dev_pm_opp_of_add_table_indexed(dev.as_raw(), index) })?;

        // Fetch the newly created table.
        let mut table = Self::from_dev(dev)?;
        table.of = true;

        Ok(table)
    }

    // Remove device tree based OPP table for the device.
    #[cfg(CONFIG_OF)]
    fn remove_of(&self) {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements. We took the reference earlier from `from_of` earlier, it is safe to drop
        // the same now.
        unsafe { bindings::dev_pm_opp_of_remove_table(self.dev.as_raw()) };
    }

    /// Add device tree based OPP table for CPU devices.
    #[cfg(CONFIG_OF)]
    pub fn from_of_cpumask(dev: &Device, cpumask: &mut Cpumask) -> Result<Self> {
        // SAFETY: The cpumask is valid and the returned ptr will be owned by the [`Table`] instance.
        to_result(unsafe { bindings::dev_pm_opp_of_cpumask_add_table(cpumask.as_raw()) })?;

        // Fetch the newly created table.
        let mut table = Self::from_dev(dev)?;

        let mut mask = Cpumask::new()?;
        cpumask.copy(&mut mask);
        table.cpumask = Some(mask);

        Ok(table)
    }

    // Remove device tree based OPP table for CPU devices.
    #[cfg(CONFIG_OF)]
    fn remove_of_cpumask(&self, mut cpumask: Cpumask) {
        // SAFETY: The cpumask is valid and we took the reference from `from_of_cpumask` earlier,
        // it is safe to drop the same now.
        unsafe { bindings::dev_pm_opp_of_cpumask_remove_table(cpumask.as_raw()) };
    }

    /// Returns the number of OPPs in the table.
    pub fn opp_count(&self) -> Result<u32> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        let ret = unsafe { bindings::dev_pm_opp_get_opp_count(self.dev.as_raw()) };
        if ret < 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(ret as u32)
        }
    }

    /// Returns max clock latency of the OPPs in the table.
    pub fn max_clock_latency(&self) -> usize {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        unsafe { bindings::dev_pm_opp_get_max_clock_latency(self.dev.as_raw()) }
    }

    /// Returns max volt latency of the OPPs in the table.
    pub fn max_volt_latency(&self) -> usize {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        unsafe { bindings::dev_pm_opp_get_max_volt_latency(self.dev.as_raw()) }
    }

    /// Returns max transition latency of the OPPs in the table.
    pub fn max_transition_latency(&self) -> usize {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        unsafe { bindings::dev_pm_opp_get_max_transition_latency(self.dev.as_raw()) }
    }

    /// Returns the suspend OPP.
    pub fn suspend_freq(&self) -> usize {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        unsafe { bindings::dev_pm_opp_get_suspend_opp_freq(self.dev.as_raw()) }
    }

    /// Synchronizes regulators used by the OPP table.
    pub fn sync_regulators(&self) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_sync_regulators(self.dev.as_raw()) })
    }

    /// Gets sharing CPUs.
    pub fn sharing_cpus(dev: &Device, cpumask: &mut Cpumask) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe {
            bindings::dev_pm_opp_get_sharing_cpus(dev.as_raw(), cpumask.as_raw())
        })
    }

    /// Sets sharing CPUs.
    pub fn set_sharing_cpus(&mut self, cpumask: &mut Cpumask) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe {
            bindings::dev_pm_opp_set_sharing_cpus(self.dev.as_raw(), cpumask.as_raw())
        })?;

        if let Some(mask) = self.cpumask.as_mut() {
            // Update the cpumask as this will be used while removing the table.
            cpumask.copy(mask);
        }

        Ok(())
    }

    /// Gets sharing CPUs from Device tree.
    #[cfg(CONFIG_OF)]
    pub fn of_sharing_cpus(dev: &Device, cpumask: &mut Cpumask) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe {
            bindings::dev_pm_opp_of_get_sharing_cpus(dev.as_raw(), cpumask.as_raw())
        })
    }

    /// Updates the voltage value for an OPP.
    pub fn adjust_voltage(
        &self,
        freq: usize,
        u_volt: usize,
        u_volt_min: usize,
        u_volt_max: usize,
    ) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe {
            bindings::dev_pm_opp_adjust_voltage(
                self.dev.as_raw(),
                freq,
                u_volt,
                u_volt_min,
                u_volt_max,
            )
        })
    }

    /// Sets a matching OPP based on frequency.
    pub fn set_rate(&self, freq: usize) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_set_rate(self.dev.as_raw(), freq) })
    }

    /// Sets exact OPP.
    pub fn set_opp(&self, opp: &OPP) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_set_opp(self.dev.as_raw(), opp.as_raw()) })
    }

    /// Finds OPP based on frequency.
    pub fn opp_from_freq(
        &self,
        mut freq: usize,
        available: Option<bool>,
        index: Option<u32>,
        stype: SearchType,
    ) -> Result<ARef<OPP>> {
        let rdev = self.dev.as_raw();
        let index = index.unwrap_or(0);

        let ptr = from_err_ptr(match stype {
            SearchType::Exact => {
                if let Some(available) = available {
                    // SAFETY: The requirements are satisfied by the existence of `Device` and
                    // its safety requirements. The returned ptr will be owned by the new [`OPP`]
                    // instance.
                    unsafe {
                        bindings::dev_pm_opp_find_freq_exact_indexed(rdev, freq, index, available)
                    }
                } else {
                    return Err(EINVAL);
                }
            }

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Ceil => unsafe {
                bindings::dev_pm_opp_find_freq_ceil_indexed(rdev, &mut freq as *mut usize, index)
            },

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Floor => unsafe {
                bindings::dev_pm_opp_find_freq_floor_indexed(rdev, &mut freq as *mut usize, index)
            },
        })?;

        // SAFETY: The `ptr` is guaranteed by the C code to be valid.
        unsafe { OPP::from_raw_opp_owned(ptr) }
    }

    /// Finds OPP based on level.
    pub fn opp_from_level(&self, mut level: u32, stype: SearchType) -> Result<ARef<OPP>> {
        let rdev = self.dev.as_raw();

        let ptr = from_err_ptr(match stype {
            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Exact => unsafe { bindings::dev_pm_opp_find_level_exact(rdev, level) },

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Ceil => unsafe {
                bindings::dev_pm_opp_find_level_ceil(rdev, &mut level as *mut u32)
            },

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Floor => unsafe {
                bindings::dev_pm_opp_find_level_floor(rdev, &mut level as *mut u32)
            },
        })?;

        // SAFETY: The `ptr` is guaranteed by the C code to be valid.
        unsafe { OPP::from_raw_opp_owned(ptr) }
    }

    /// Finds OPP based on bandwidth.
    pub fn opp_from_bw(&self, mut bw: u32, index: i32, stype: SearchType) -> Result<ARef<OPP>> {
        let rdev = self.dev.as_raw();

        let ptr = from_err_ptr(match stype {
            // The OPP core doesn't support this yet.
            SearchType::Exact => return Err(EINVAL),

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Ceil => unsafe {
                bindings::dev_pm_opp_find_bw_ceil(rdev, &mut bw as *mut u32, index)
            },

            // SAFETY: The requirements are satisfied by the existence of `Device` and its
            // safety requirements. The returned ptr will be owned by the new [`OPP`] instance.
            SearchType::Floor => unsafe {
                bindings::dev_pm_opp_find_bw_floor(rdev, &mut bw as *mut u32, index)
            },
        })?;

        // SAFETY: The `ptr` is guaranteed by the C code to be valid.
        unsafe { OPP::from_raw_opp_owned(ptr) }
    }

    /// Enable the OPP.
    pub fn enable_opp(&self, freq: usize) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_enable(self.dev.as_raw(), freq) })
    }

    /// Disable the OPP.
    pub fn disable_opp(&self, freq: usize) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe { bindings::dev_pm_opp_disable(self.dev.as_raw(), freq) })
    }

    /// Registers with Energy model.
    #[cfg(CONFIG_OF)]
    pub fn of_register_em(&mut self, cpumask: &mut Cpumask) -> Result<()> {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements.
        to_result(unsafe {
            bindings::dev_pm_opp_of_register_em(self.dev.as_raw(), cpumask.as_raw())
        })?;

        self.em = true;
        Ok(())
    }

    // Unregisters with Energy model.
    #[cfg(all(CONFIG_OF, CONFIG_ENERGY_MODEL))]
    fn of_unregister_em(&self) {
        // SAFETY: The requirements are satisfied by the existence of `Device` and its safety
        // requirements. We registered with the EM framework earlier, it is safe to unregister now.
        unsafe { bindings::em_dev_unregister_perf_domain(self.dev.as_raw()) };
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe
        // to relinquish it now.
        unsafe { bindings::dev_pm_opp_put_opp_table(self.ptr) };

        #[cfg(CONFIG_OF)]
        {
            #[cfg(CONFIG_ENERGY_MODEL)]
            if self.em {
                self.of_unregister_em();
            }

            if self.of {
                self.remove_of();
            } else if let Some(cpumask) = self.cpumask.take() {
                self.remove_of_cpumask(cpumask);
            }
        }
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
