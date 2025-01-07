// SPDX-License-Identifier: GPL-2.0

//! CPU frequency scaling.
//!
//! This module provides bindings for interacting with the cpufreq subsystem.
//!
//! C header: [`include/linux/cpufreq.h`](srctree/include/linux/cpufreq.h)

use crate::{
    bindings,
    error::{code::*, to_result, Result},
    prelude::*,
};

use core::{
    pin::Pin,
};

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
