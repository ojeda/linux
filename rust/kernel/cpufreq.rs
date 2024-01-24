// SPDX-License-Identifier: GPL-2.0

//! CPU frequency scaling.
//!
//! This module provides bindings for interacting with the cpufreq subsystem.
//!
//! C header: [`include/linux/cpufreq.h`](srctree/include/linux/cpufreq.h)

use crate::{
    bindings, clk, cpumask,
    device::Device,
    devres::Devres,
    error::{code::*, from_err_ptr, from_result, to_result, Result, VTABLE_DEFAULT_ERROR},
    prelude::*,
    types::ForeignOwnable,
};

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    pin::Pin,
    ptr::{self, addr_of_mut},
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

/// Registration of a cpufreq driver.
pub struct Registration<T: Driver> {
    drv: KBox<UnsafeCell<bindings::cpufreq_driver>>,
    _p: PhantomData<T>,
}

// SAFETY: `Registration` doesn't offer any methods or access to fields when shared between threads
// or CPUs, so it is safe to share it.
unsafe impl<T: Driver> Sync for Registration<T> {}

#[allow(clippy::non_send_fields_in_send_ty)]
// SAFETY: Registration with and unregistration from the cpufreq subsystem can happen from any thread.
// Additionally, `T::Data` (which is dropped during unregistration) is `Send`, so it is okay to move
// `Registration` to different threads.
unsafe impl<T: Driver> Send for Registration<T> {}

impl<T: Driver> Registration<T> {
    /// Registers a cpufreq driver with the rest of the kernel.
    pub fn new(name: &'static CStr, data: T::Data, flags: u16, boost: bool) -> Result<Self> {
        let mut drv = KBox::new(
            UnsafeCell::new(bindings::cpufreq_driver::default()),
            GFP_KERNEL,
        )?;
        let drv_ref = drv.get_mut();

        // Account for the trailing null character.
        let len = name.len() + 1;
        if len > drv_ref.name.len() {
            return Err(EINVAL);
        };

        // SAFETY: `name` is a valid Cstr, and we are copying it to an array of equal or larger
        // size.
        let name = unsafe { &*(name.as_bytes_with_nul() as *const [u8]) };
        drv_ref.name[..len].copy_from_slice(name);

        drv_ref.boost_enabled = boost;
        drv_ref.flags = flags;

        // Allocate an array of 3 pointers to be passed to the C code.
        let mut attr = KBox::new([ptr::null_mut(); 3], GFP_KERNEL)?;
        let mut next = 0;

        // SAFETY: The C code returns a valid pointer here, which is again passed to the C code in
        // an array.
        attr[next] = unsafe {
            addr_of_mut!(bindings::cpufreq_freq_attr_scaling_available_freqs) as *mut _
        };
        next += 1;

        if boost {
            // SAFETY: The C code returns a valid pointer here, which is again passed to the C code
            // in an array.
            attr[next] = unsafe {
                addr_of_mut!(bindings::cpufreq_freq_attr_scaling_boost_freqs) as *mut _
            };
            next += 1;
        }
        attr[next] = ptr::null_mut();

        // Pass the ownership of the memory block to the C code. This will be freed when
        // the [`Registration`] object goes out of scope.
        drv_ref.attr = KBox::leak(attr) as *mut _;

        // Initialize mandatory callbacks.
        drv_ref.init = Some(Self::init_callback);
        drv_ref.verify = Some(Self::verify_callback);

        // Initialize optional callbacks.
        drv_ref.setpolicy = if T::HAS_SETPOLICY {
            Some(Self::setpolicy_callback)
        } else {
            None
        };
        drv_ref.target = if T::HAS_TARGET {
            Some(Self::target_callback)
        } else {
            None
        };
        drv_ref.target_index = if T::HAS_TARGET_INDEX {
            Some(Self::target_index_callback)
        } else {
            None
        };
        drv_ref.fast_switch = if T::HAS_FAST_SWITCH {
            Some(Self::fast_switch_callback)
        } else {
            None
        };
        drv_ref.adjust_perf = if T::HAS_ADJUST_PERF {
            Some(Self::adjust_perf_callback)
        } else {
            None
        };
        drv_ref.get_intermediate = if T::HAS_GET_INTERMEDIATE {
            Some(Self::get_intermediate_callback)
        } else {
            None
        };
        drv_ref.target_intermediate = if T::HAS_TARGET_INTERMEDIATE {
            Some(Self::target_intermediate_callback)
        } else {
            None
        };
        drv_ref.get = if T::HAS_GET {
            Some(Self::get_callback)
        } else {
            None
        };
        drv_ref.update_limits = if T::HAS_UPDATE_LIMITS {
            Some(Self::update_limits_callback)
        } else {
            None
        };
        drv_ref.bios_limit = if T::HAS_BIOS_LIMIT {
            Some(Self::bios_limit_callback)
        } else {
            None
        };
        drv_ref.online = if T::HAS_ONLINE {
            Some(Self::online_callback)
        } else {
            None
        };
        drv_ref.offline = if T::HAS_OFFLINE {
            Some(Self::offline_callback)
        } else {
            None
        };
        drv_ref.exit = if T::HAS_EXIT {
            Some(Self::exit_callback)
        } else {
            None
        };
        drv_ref.suspend = if T::HAS_SUSPEND {
            Some(Self::suspend_callback)
        } else {
            None
        };
        drv_ref.resume = if T::HAS_RESUME {
            Some(Self::resume_callback)
        } else {
            None
        };
        drv_ref.ready = if T::HAS_READY {
            Some(Self::ready_callback)
        } else {
            None
        };
        drv_ref.set_boost = if T::HAS_SET_BOOST {
            Some(Self::set_boost_callback)
        } else {
            None
        };
        drv_ref.register_em = if T::HAS_REGISTER_EM {
            Some(Self::register_em_callback)
        } else {
            None
        };

        // Set driver data before registering the driver, as the cpufreq core may call few
        // callbacks before `cpufreq_register_driver()` returns.
        Self::set_data(drv_ref, data)?;

        // SAFETY: It is safe to register the driver with the cpufreq core in the C code.
        to_result(unsafe { bindings::cpufreq_register_driver(drv_ref) })?;

        Ok(Self {
            drv,
            _p: PhantomData,
        })
    }

    /// Same as [Registration::new`], but does not return a `Registration` instance.
    /// Instead the `Registration` is owned by devres and will be revoked / dropped, once the
    /// device is detached.
    pub fn new_foreign_owned(
        dev: &Device,
        name: &'static CStr,
        data: T::Data,
        flags: u16,
        boost: bool,
    ) -> Result<()> {
        let reg = Self::new(name, data, flags, boost)?;
        Devres::new_foreign_owned(dev, reg, GFP_KERNEL)?;
        Ok(())
    }

    // Sets the data for a cpufreq driver.
    fn set_data(drv: &mut bindings::cpufreq_driver, data: T::Data) -> Result<()> {
        if drv.driver_data.is_null() {
            // Pass the ownership of the data to the foreign interface.
            drv.driver_data = <T::Data as ForeignOwnable>::into_foreign(data) as _;
            Ok(())
        } else {
            Err(EBUSY)
        }
    }

    /// Returns the previous set data for a cpufreq driver.
    pub fn data(&mut self) -> Option<<T::Data as ForeignOwnable>::Borrowed<'static>> {
        let drv = self.drv.get_mut();

        if drv.driver_data.is_null() {
            None
        } else {
            // SAFETY: The data is earlier set by us from [`set_data()`].
            Some(unsafe { <T::Data as ForeignOwnable>::borrow(drv.driver_data) })
        }
    }

    // Clears and returns the data for a cpufreq driver.
    fn clear_data(&mut self) -> Option<T::Data> {
        let drv = self.drv.get_mut();

        if drv.driver_data.is_null() {
            None
        } else {
            // SAFETY: By the type invariants, we know that `self` owns a reference, so it is safe to
            // relinquish it now.
            let data = Some(unsafe { <T::Data as ForeignOwnable>::from_foreign(drv.driver_data) });
            drv.driver_data = ptr::null_mut();
            data
        }
    }
}

// cpufreq driver callbacks.
impl<T: Driver> Registration<T> {
    // Policy's init callback.
    extern "C" fn init_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };

            let data = T::init(&mut policy)?;
            policy.set_data(data)?;
            Ok(0)
        })
    }

    // Policy's exit callback.
    extern "C" fn exit_callback(ptr: *mut bindings::cpufreq_policy) {
        // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
        // duration of this call, so it is guaranteed to remain alive for the lifetime of
        // `ptr`.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };

        let data = policy.clear_data();
        let _ = T::exit(&mut policy, data);
    }

    // Policy's online callback.
    extern "C" fn online_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::online(&mut policy).map(|()| 0)
        })
    }

    // Policy's offline callback.
    extern "C" fn offline_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::offline(&mut policy).map(|()| 0)
        })
    }

    // Policy's suspend callback.
    extern "C" fn suspend_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::suspend(&mut policy).map(|()| 0)
        })
    }

    // Policy's resume callback.
    extern "C" fn resume_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::resume(&mut policy).map(|()| 0)
        })
    }

    // Policy's ready callback.
    extern "C" fn ready_callback(ptr: *mut bindings::cpufreq_policy) {
        // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
        // duration of this call, so it is guaranteed to remain alive for the lifetime of
        // `ptr`.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };
        T::ready(&mut policy);
    }

    // Policy's verify callback.
    extern "C" fn verify_callback(ptr: *mut bindings::cpufreq_policy_data) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut data = unsafe { PolicyData::from_raw_policy_data(ptr) };
            T::verify(&mut data).map(|()| 0)
        })
    }

    // Policy's setpolicy callback.
    extern "C" fn setpolicy_callback(ptr: *mut bindings::cpufreq_policy) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::setpolicy(&mut policy).map(|()| 0)
        })
    }

    // Policy's target callback.
    extern "C" fn target_callback(
        ptr: *mut bindings::cpufreq_policy,
        target_freq: u32,
        relation: u32,
    ) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::target(&mut policy, target_freq, Relation::new(relation)?).map(|()| 0)
        })
    }

    // Policy's target_index callback.
    extern "C" fn target_index_callback(
        ptr: *mut bindings::cpufreq_policy,
        index: u32,
    ) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::target_index(&mut policy, index).map(|()| 0)
        })
    }

    // Policy's fast_switch callback.
    extern "C" fn fast_switch_callback(
        ptr: *mut bindings::cpufreq_policy,
        target_freq: u32,
    ) -> core::ffi::c_uint {
        // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
        // duration of this call, so it is guaranteed to remain alive for the lifetime of
        // `ptr`.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };
        T::fast_switch(&mut policy, target_freq)
    }

    // Policy's adjust_perf callback.
    extern "C" fn adjust_perf_callback(cpu: u32, min_perf: usize, target_perf: usize, capacity: usize) {
        if let Ok(mut policy) = Policy::from_cpu(cpu) {
            T::adjust_perf(&mut policy, min_perf, target_perf, capacity);
        }
    }

    // Policy's get_intermediate callback.
    extern "C" fn get_intermediate_callback(
        ptr: *mut bindings::cpufreq_policy,
        index: u32,
    ) -> core::ffi::c_uint {
        // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
        // duration of this call, so it is guaranteed to remain alive for the lifetime of
        // `ptr`.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };
        T::get_intermediate(&mut policy, index)
    }

    // Policy's target_intermediate callback.
    extern "C" fn target_intermediate_callback(
        ptr: *mut bindings::cpufreq_policy,
        index: u32,
    ) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::target_intermediate(&mut policy, index).map(|()| 0)
        })
    }

    // Policy's get callback.
    extern "C" fn get_callback(cpu: u32) -> core::ffi::c_uint {
        Policy::from_cpu(cpu).map_or(0, |mut policy| T::get(&mut policy).map_or(0, |f| f))
    }

    // Policy's update_limit callback.
    extern "C" fn update_limits_callback(cpu: u32) {
        if let Ok(mut policy) = Policy::from_cpu(cpu) {
            T::update_limits(&mut policy);
        }
    }

    // Policy's bios_limit callback.
    extern "C" fn bios_limit_callback(cpu: i32, limit: *mut u32) -> core::ffi::c_int {
        from_result(|| {
            let mut policy = Policy::from_cpu(cpu as u32)?;

            // SAFETY: The pointer is guaranteed by the C code to be valid.
            T::bios_limit(&mut policy, &mut (unsafe { *limit })).map(|()| 0)
        })
    }

    // Policy's set_boost callback.
    extern "C" fn set_boost_callback(
        ptr: *mut bindings::cpufreq_policy,
        state: i32,
    ) -> core::ffi::c_int {
        from_result(|| {
            // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
            // duration of this call, so it is guaranteed to remain alive for the lifetime of
            // `ptr`.
            let mut policy = unsafe { Policy::from_raw_policy(ptr) };
            T::set_boost(&mut policy, state).map(|()| 0)
        })
    }

    // Policy's register_em callback.
    extern "C" fn register_em_callback(ptr: *mut bindings::cpufreq_policy) {
        // SAFETY: `ptr` is valid by the contract with the C code. `policy` is alive only for the
        // duration of this call, so it is guaranteed to remain alive for the lifetime of
        // `ptr`.
        let mut policy = unsafe { Policy::from_raw_policy(ptr) };
        T::register_em(&mut policy);
    }
}

impl<T: Driver> Drop for Registration<T> {
    // Removes the registration from the kernel if it has completed successfully before.
    fn drop(&mut self) {
        pr_info!("Registration dropped\n");
        let drv = self.drv.get_mut();

        // SAFETY: The driver was earlier registered from `new()`.
        unsafe { bindings::cpufreq_unregister_driver(drv) };

        // Free the previously leaked memory to the C code.
        if !drv.attr.is_null() {
            // SAFETY: The pointer was earlier initialized from the result of `KBox::leak`.
            unsafe { drop(KBox::from_raw(drv.attr)) };
        }

        // Free data
        drop(self.clear_data());
    }
}
