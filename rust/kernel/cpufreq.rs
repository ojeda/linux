// SPDX-License-Identifier: GPL-2.0

//! CPU frequency scaling.
//!
//! This module provides bindings for interacting with the cpufreq subsystem.
//!
//! C header: [`include/linux/cpufreq.h`](srctree/include/linux/cpufreq.h)

use crate::{
    bindings, clk, cpumask,
    device::Device,
    error::{code::*, from_err_ptr, to_result, Result, VTABLE_DEFAULT_ERROR},
    prelude::*,
    types::ForeignOwnable,
};

use core::{
    pin::Pin,
    ptr::self,
};

use macros::vtable;

/// Default transition latency value.
pub const ETERNAL_LATENCY: u32 = bindings::CPUFREQ_ETERNAL as u32;

/// Container for cpufreq driver flags.
pub mod flags {
    use crate::bindings;

    /// Set by drivers that need to update internal upper and lower boundaries along with the
    /// target frequency and so the core and governors should also invoke the driver if the target
    /// frequency does not change, but the policy min or max may have changed.
    pub const NEED_UPDATE_LIMITS: u16 = bindings::CPUFREQ_NEED_UPDATE_LIMITS as _;

    /// Set by drivers for platforms where loops_per_jiffy or other kernel "constants" aren't
    /// affected by frequency transitions.
    pub const CONST_LOOPS: u16 = bindings::CPUFREQ_CONST_LOOPS as _;

    /// Set by drivers that want the core to automatically register the cpufreq driver as a thermal
    /// cooling device.
    pub const IS_COOLING_DEV: u16 = bindings::CPUFREQ_IS_COOLING_DEV as _;

    /// Set by drivers for platforms that have multiple clock-domains, i.e. supporting multiple
    /// policies. With this sysfs directories of governor would be created in cpu/cpuN/cpufreq/
    /// directory and so they can use the same governor with different tunables for different
    /// clusters.
    pub const HAVE_GOVERNOR_PER_POLICY: u16 = bindings::CPUFREQ_HAVE_GOVERNOR_PER_POLICY as _;

    /// Set by drivers which do POSTCHANGE notifications from outside of their ->target() routine.
    pub const ASYNC_NOTIFICATION: u16 = bindings::CPUFREQ_ASYNC_NOTIFICATION as _;

    /// Set by drivers that want cpufreq core to check if CPU is running at a frequency present in
    /// freq-table exposed by the driver. For these drivers if CPU is found running at an out of
    /// table freq, the cpufreq core will try to change the frequency to a value from the table.
    /// And if that fails, it will stop further boot process by issuing a BUG_ON().
    pub const NEED_INITIAL_FREQ_CHECK: u16 = bindings::CPUFREQ_NEED_INITIAL_FREQ_CHECK as _;

    /// Set by drivers to disallow use of governors with "dynamic_switching" flag set.
    pub const NO_AUTO_DYNAMIC_SWITCHING: u16 = bindings::CPUFREQ_NO_AUTO_DYNAMIC_SWITCHING as _;
}

/// CPU frequency selection relations. Each value contains a `bool` argument which corresponds to
/// the Relation being efficient.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Relation {
    /// Select the lowest frequency at or above target.
    Low(bool),
    /// Select the highest frequency below or at target.
    High(bool),
    /// Select the closest frequency to the target.
    Close(bool),
}

impl Relation {
    // Converts from a value compatible with the C code.
    fn new(val: u32) -> Result<Self> {
        let efficient = val & bindings::CPUFREQ_RELATION_E != 0;

        Ok(match val & !bindings::CPUFREQ_RELATION_E {
            bindings::CPUFREQ_RELATION_L => Self::Low(efficient),
            bindings::CPUFREQ_RELATION_H => Self::High(efficient),
            bindings::CPUFREQ_RELATION_C => Self::Close(efficient),
            _ => return Err(EINVAL),
        })
    }

    /// Converts to a value compatible with the C code.
    pub fn val(&self) -> u32 {
        let (mut val, e) = match self {
            Self::Low(e) => (bindings::CPUFREQ_RELATION_L, e),
            Self::High(e) => (bindings::CPUFREQ_RELATION_H, e),
            Self::Close(e) => (bindings::CPUFREQ_RELATION_C, e),
        };

        if *e {
            val |= bindings::CPUFREQ_RELATION_E;
        }

        val
    }
}

/// Equivalent to `struct cpufreq_policy_data` in the C code.
#[repr(transparent)]
pub struct PolicyData(*mut bindings::cpufreq_policy_data);

impl PolicyData {
    /// Creates new instance of [`PolicyData`].
    ///
    /// # Safety
    ///
    /// Callers must ensure that `ptr` is valid and non-null.
    pub unsafe fn from_raw_policy_data(ptr: *mut bindings::cpufreq_policy_data) -> Self {
        Self(ptr)
    }

    /// Returns the raw pointer to the C structure.
    #[inline]
    pub fn as_raw(&self) -> *mut bindings::cpufreq_policy_data {
        self.0
    }

    /// Provides a wrapper to the generic verify routine.
    pub fn generic_verify(&self) -> Result<()> {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it now.
        to_result(unsafe { bindings::cpufreq_generic_frequency_table_verify(self.as_raw()) })
    }
}

/// Builder for the `struct cpufreq_frequency_table` in the C code.
#[repr(transparent)]
#[derive(Default)]
pub struct TableBuilder {
    entries: KVec<bindings::cpufreq_frequency_table>,
}

impl TableBuilder {
    /// Creates new instance of [`TableBuilder`].
    pub fn new() -> Self {
        Self {
            entries: KVec::new(),
        }
    }

    /// Adds a new entry to the table.
    pub fn add(&mut self, frequency: u32, flags: u32, driver_data: u32) -> Result<()> {
        // Adds new entry to the end of the vector.
        Ok(self.entries.push(
            bindings::cpufreq_frequency_table {
                flags,
                driver_data,
                frequency,
            },
            GFP_KERNEL,
        )?)
    }

    /// Creates [`Table`] from [`TableBuilder`].
    pub fn into_table(mut self) -> Result<Table> {
        // Add last entry to the table.
        self.add(bindings::CPUFREQ_TABLE_END as u32, 0, 0)?;
        Table::from_builder(self.entries)
    }
}

/// A simple implementation of the cpufreq table, equivalent to the `struct
/// cpufreq_frequency_table` in the C code.
pub struct Table {
    #[allow(dead_code)]
    // Dynamically created table.
    entries: Option<Pin<KVec<bindings::cpufreq_frequency_table>>>,

    // Pointer to the statically or dynamically created table.
    ptr: *mut bindings::cpufreq_frequency_table,
}

impl Table {
    /// Creates new instance of [`Table`] from [`TableBuilder`].
    fn from_builder(entries: KVec<bindings::cpufreq_frequency_table>) -> Result<Self> {
        if entries.is_empty() {
            return Err(EINVAL);
        }

        // Pin the entries to memory, since we are passing its pointer to the C code.
        let mut entries = Pin::new(entries);

        // The pointer is valid until the table gets dropped.
        let ptr = entries.as_mut_ptr();

        Ok(Self {
            entries: Some(entries),
            ptr,
        })
    }

    /// Creates new instance of [`Table`] from raw pointer.
    ///
    /// # Safety
    ///
    /// Callers must ensure that `ptr` is valid and non-null for the lifetime of the [`Table`].
    pub unsafe fn from_raw(ptr: *mut bindings::cpufreq_frequency_table) -> Self {
        Self {
            entries: None,
            ptr,
        }
    }

    /// Returns raw pointer to the `struct cpufreq_frequency_table` compatible with the C code.
    #[inline]
    pub fn as_raw(&self) -> *mut bindings::cpufreq_frequency_table {
        self.ptr
    }

    /// Returns `frequency` at index in the [`Table`].
    pub fn freq(&self, index: usize) -> Result<u32> {
        // SAFETY: The pointer is guaranteed to be valid for the lifetime of `self` and `index` is
        // guaranteed to be within limits of the frequency table by the C API.
        Ok(unsafe { (*self.ptr.add(index)).frequency })
    }

    /// Returns `flags` at index in the [`Table`].
    pub fn flags(&self, index: usize) -> Result<u32> {
        // SAFETY: The pointer is guaranteed to be valid for the lifetime of `self` and `index` is
        // guaranteed to be within limits of the frequency table by the C API.
        Ok(unsafe { (*self.ptr.add(index)).flags })
    }

    /// Returns `data` at index in the [`Table`].
    pub fn data(&self, index: usize) -> Result<u32> {
        // SAFETY: The pointer is guaranteed to be valid for the lifetime of `self` and `index` is
        // guaranteed to be within limits of the frequency table by the C API.
        Ok(unsafe { (*self.ptr.add(index)).driver_data })
    }
}

/// Equivalent to `struct cpufreq_policy` in the C code.
pub struct Policy {
    ptr: *mut bindings::cpufreq_policy,
    put_cpu: bool,
    cpumask: cpumask::Cpumask,
}

impl Policy {
    /// Creates a new instance of [`Policy`].
    ///
    /// # Safety
    ///
    /// Callers must ensure that `ptr` is valid and non-null.
    pub unsafe fn from_raw_policy(ptr: *mut bindings::cpufreq_policy) -> Self {
        Self {
            ptr,
            put_cpu: false,
            // SAFETY: The pointer is guaranteed to be valid for the lifetime of `Self`. The `cpus`
            // pointer is guaranteed to be valid by the C code.
            cpumask: unsafe { cpumask::Cpumask::get_cpumask((*ptr).cpus) },
        }
    }

    fn from_cpu(cpu: u32) -> Result<Self> {
        // SAFETY: It is safe to call `cpufreq_cpu_get()` for any CPU.
        let ptr = from_err_ptr(unsafe { bindings::cpufreq_cpu_get(cpu) })?;

        // SAFETY: The pointer is guaranteed to be valid by the C code.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };
        policy.put_cpu = true;
        Ok(policy)
    }

    /// Raw pointer to the underlying cpufreq policy.
    #[inline]
    pub fn as_raw(&self) -> *mut bindings::cpufreq_policy {
        self.ptr
    }

    fn as_ref(&self) -> &bindings::cpufreq_policy {
        // SAFETY: By the type invariants, we know that `self` owns a reference to the pointer.
        unsafe { &(*self.ptr) }
    }
    fn as_mut_ref(&mut self) -> &mut bindings::cpufreq_policy {
        // SAFETY: By the type invariants, we know that `self` owns a reference to the pointer.
        unsafe { &mut (*self.ptr) }
    }

    /// Returns the primary CPU for a cpufreq policy.
    pub fn cpu(&self) -> u32 {
        self.as_ref().cpu
    }

    /// Returns the minimum frequency for a cpufreq policy.
    pub fn min(&self) -> u32 {
        self.as_ref().min
    }

    /// Set the minimum frequency for a cpufreq policy.
    pub fn set_min(&mut self, min: u32) -> &mut Self {
        self.as_mut_ref().min = min;
        self
    }

    /// Returns the maximum frequency for a cpufreq policy.
    pub fn max(&self) -> u32 {
        self.as_ref().max
    }

    /// Set the maximum frequency for a cpufreq policy.
    pub fn set_max(&mut self, max: u32) -> &mut Self {
        self.as_mut_ref().max = max;
        self
    }

    /// Returns the current frequency for a cpufreq policy.
    pub fn cur(&self) -> u32 {
        self.as_ref().cur
    }

    /// Sets the suspend frequency for a cpufreq policy.
    pub fn set_suspend_freq(&mut self, freq: u32) -> &mut Self {
        self.as_mut_ref().suspend_freq = freq;
        self
    }

    /// Returns the suspend frequency for a cpufreq policy.
    pub fn suspend_freq(&self) -> u32 {
        self.as_ref().suspend_freq
    }

    /// Provides a wrapper to the generic suspend routine.
    pub fn generic_suspend(&self) -> Result<()> {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it now.
        to_result(unsafe { bindings::cpufreq_generic_suspend(self.as_raw()) })
    }

    /// Provides a wrapper to the generic get routine.
    pub fn generic_get(&self) -> Result<u32> {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it now.
        Ok(unsafe { bindings::cpufreq_generic_get(self.cpu()) })
    }

    /// Provides a wrapper to the register em with OPP routine.
    pub fn register_em_opp(&self) {
        // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
        // use it now.
        unsafe { bindings::cpufreq_register_em_with_opp(self.as_raw()) };
    }

    /// Gets raw pointer to cpufreq policy's CPUs mask.
    pub fn cpus(&mut self) -> &mut cpumask::Cpumask {
        &mut self.cpumask
    }

    /// Sets clock for a cpufreq policy.
    pub fn set_clk(&mut self, dev: &Device, name: Option<&CStr>) -> Result<clk::Clk> {
        let clk = clk::Clk::new(dev, name)?;
        self.as_mut_ref().clk = clk.as_raw();
        Ok(clk)
    }

    /// Allows frequency switching code to run on any CPU.
    pub fn set_dvfs_possible_from_any_cpu(&mut self) -> &mut Self {
        self.as_mut_ref().dvfs_possible_from_any_cpu = true;
        self
    }

    /// Get fast_switch_possible value.
    pub fn fast_switch_possible(&self) -> bool {
        self.as_ref().fast_switch_possible
    }

    /// Enable/disable fast frequency switching.
    pub fn set_fast_switch_possible(&mut self, val: bool) -> &mut Self {
        self.as_mut_ref().fast_switch_possible = val;
        self
    }

    /// Sets transition latency for a cpufreq policy.
    pub fn set_transition_latency(&mut self, latency: u32) -> &mut Self {
        self.as_mut_ref().cpuinfo.transition_latency = latency;
        self
    }

    /// Set cpuinfo.min_freq.
    pub fn set_cpuinfo_min_freq(&mut self, min_freq: u32) -> &mut Self {
        self.as_mut_ref().cpuinfo.min_freq = min_freq;
        self
    }

    /// Set cpuinfo.max_freq.
    pub fn set_cpuinfo_max_freq(&mut self, max_freq: u32) -> &mut Self {
        self.as_mut_ref().cpuinfo.max_freq = max_freq;
        self
    }

    /// Set transition_delay_us, i.e. time between successive freq. change requests.
    pub fn set_transition_delay_us(&mut self, transition_delay_us: u32) -> &mut Self {
        self.as_mut_ref().transition_delay_us = transition_delay_us;
        self
    }

    /// Returns the cpufreq table for a cpufreq policy. The cpufreq table is recreated in a
    /// light-weight manner from the raw pointer. The table in C code is not freed once this table
    /// is dropped.
    pub fn freq_table(&self) -> Result<Table> {
        if self.as_ref().freq_table.is_null() {
            return Err(EINVAL);
        }

        // SAFETY: The `freq_table` is guaranteed to be valid.
        Ok(unsafe { Table::from_raw(self.as_ref().freq_table) })
    }

    /// Sets the cpufreq table for a cpufreq policy.
    ///
    /// The cpufreq driver must guarantee that the frequency table does not get freed while it is
    /// still being used by the C code.
    pub fn set_freq_table(&mut self, table: &Table) -> &mut Self {
        self.as_mut_ref().freq_table = table.as_raw();
        self
    }

    /// Returns the data for a cpufreq policy.
    pub fn data<T: ForeignOwnable>(&mut self) -> Option<<T>::Borrowed<'_>> {
        if self.as_ref().driver_data.is_null() {
            None
        } else {
            // SAFETY: The data is earlier set by us from [`set_data()`].
            Some(unsafe { T::borrow(self.as_ref().driver_data) })
        }
    }

    // Sets the data for a cpufreq policy.
    fn set_data<T: ForeignOwnable>(&mut self, data: T) -> Result<()> {
        if self.as_ref().driver_data.is_null() {
            // Pass the ownership of the data to the foreign interface.
            self.as_mut_ref().driver_data = <T as ForeignOwnable>::into_foreign(data) as _;
            Ok(())
        } else {
            Err(EBUSY)
        }
    }

    // Returns the data for a cpufreq policy.
    fn clear_data<T: ForeignOwnable>(&mut self) -> Option<T> {
        if self.as_ref().driver_data.is_null() {
            None
        } else {
            let data = Some(
                // SAFETY: The data is earlier set by us from [`set_data()`]. It is safe to take
                // back the ownership of the data from the foreign interface.
                unsafe { <T as ForeignOwnable>::from_foreign(self.as_ref().driver_data) }
            );
            self.as_mut_ref().driver_data = ptr::null_mut();
            data
        }
    }
}

impl Drop for Policy {
    fn drop(&mut self) {
        if self.put_cpu {
            // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
            // relinquish it now.
            unsafe { bindings::cpufreq_cpu_put(self.as_raw()) };
        }
    }
}

/// Operations to be implemented by a cpufreq driver.
#[vtable]
pub trait Driver {
    /// Driver specific data.
    ///
    /// Corresponds to the data retrieved via the kernel's
    /// `cpufreq_get_driver_data()` function.
    ///
    /// Require that `Data` implements `ForeignOwnable`. We guarantee to
    /// never move the underlying wrapped data structure.
    type Data: ForeignOwnable;

    /// Policy specific data.
    ///
    /// Require that `PData` implements `ForeignOwnable`. We guarantee to
    /// never move the underlying wrapped data structure.
    type PData: ForeignOwnable;

    /// Policy's init callback.
    fn init(policy: &mut Policy) -> Result<Self::PData>;

    /// Policy's exit callback.
    fn exit(_policy: &mut Policy, _data: Option<Self::PData>) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's online callback.
    fn online(_policy: &mut Policy) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's offline callback.
    fn offline(_policy: &mut Policy) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's suspend callback.
    fn suspend(_policy: &mut Policy) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's resume callback.
    fn resume(_policy: &mut Policy) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's ready callback.
    fn ready(_policy: &mut Policy) {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's verify callback.
    fn verify(data: &mut PolicyData) -> Result<()>;

    /// Policy's setpolicy callback.
    fn setpolicy(_policy: &mut Policy) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's target callback.
    fn target(_policy: &mut Policy, _target_freq: u32, _relation: Relation) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's target_index callback.
    fn target_index(_policy: &mut Policy, _index: u32) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's fast_switch callback.
    fn fast_switch(_policy: &mut Policy, _target_freq: u32) -> u32 {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's adjust_perf callback.
    fn adjust_perf(_policy: &mut Policy, _min_perf: usize, _target_perf: usize, _capacity: usize) {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's get_intermediate callback.
    fn get_intermediate(_policy: &mut Policy, _index: u32) -> u32 {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's target_intermediate callback.
    fn target_intermediate(_policy: &mut Policy, _index: u32) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's get callback.
    fn get(_policy: &mut Policy) -> Result<u32> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's update_limits callback.
    fn update_limits(_policy: &mut Policy) {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's bios_limit callback.
    fn bios_limit(_policy: &mut Policy, _limit: &mut u32) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's set_boost callback.
    fn set_boost(_policy: &mut Policy, _state: i32) -> Result<()> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    /// Policy's register_em callback.
    fn register_em(_policy: &mut Policy) {
        build_error!(VTABLE_DEFAULT_ERROR)
    }
}
