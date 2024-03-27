// SPDX-License-Identifier: GPL-2.0

//! Rust based implementation of the cpufreq-dt driver.

use core::format_args;

use kernel::{
    c_str, clk, cpu, cpufreq, cpumask::Cpumask, device::Device,
    error::code::*, fmt, macros::vtable, module_platform_driver, of, opp, platform, prelude::*,
    str::CString, sync::Arc,
};

// Finds exact supply name from the OF node.
fn find_supply_name_exact(dev: &Device, name: &str) -> Option<CString> {
    let name_cstr = CString::try_from_fmt(fmt!("{}-supply", name)).ok()?;

    if dev.property_present(&name_cstr) {
        CString::try_from_fmt(fmt!("{}", name)).ok()
    } else {
        None
    }
}

// Finds supply name for the CPU from DT.
fn find_supply_names(dev: &Device, cpu: u32) -> Option<KVec<CString>> {
    // Try "cpu0" for older DTs.
    let name = match cpu {
        0 => find_supply_name_exact(dev, "cpu0"),
        _ => None,
    }
    .or(find_supply_name_exact(dev, "cpu"))?;

    let mut list = KVec::with_capacity(1, GFP_KERNEL).ok()?;
    list.push(name, GFP_KERNEL).ok()?;

    Some(list)
}

// Represents the cpufreq dt device.
struct CPUFreqDTDevice {
    opp_table: opp::Table,
    freq_table: opp::FreqTable,
    #[allow(dead_code)]
    mask: Cpumask,
    #[allow(dead_code)]
    token: Option<opp::ConfigToken>,
    #[allow(dead_code)]
    clk: clk::Clk,
}

struct CPUFreqDTDriver {
    _pdev: platform::Device,
}

#[vtable]
impl opp::ConfigOps for CPUFreqDTDriver {}

#[vtable]
impl cpufreq::Driver for CPUFreqDTDriver {
    type Data = ();
    type PData = Arc<CPUFreqDTDevice>;

    fn init(policy: &mut cpufreq::Policy) -> Result<Self::PData> {
        pr_info!("init\n");
        let cpu = policy.cpu();
        // SAFETY: The CPU device is only used from the init() callback during which the CPU can't
        // get hot-unplugged. Also the cpufreq core in C library, registers with CPU notifiers and
        // the cpufreq core/driver won't use the CPU device, once the CPU is hot-unplugged.
        let dev = unsafe { cpu::from_cpu(cpu)? };
        let mut mask = Cpumask::new()?;

        mask.set(cpu);

        let token = match find_supply_names(dev, cpu) {
            Some(names) => Some(
                opp::Config::<Self>::new()
                    .set_regulator_names(names)?
                    .set(dev)?,
            ),
            _ => None,
        };

        // Get OPP-sharing information from "operating-points-v2" bindings.
        let fallback = match opp::Table::of_sharing_cpus(dev, &mut mask) {
            Ok(()) => false,
            Err(e) => {
                if e != ENOENT {
                    return Err(e);
                }

                // "operating-points-v2" not supported. If the platform hasn't
                // set sharing CPUs, fallback to all CPUs share the `Policy`
                // for backward compatibility.
                opp::Table::sharing_cpus(dev, &mut mask).is_err()
            }
        };

        // Initialize OPP tables for all policy cpus.
        //
        // For platforms not using "operating-points-v2" bindings, we do this
        // before updating policy cpus. Otherwise, we will end up creating
        // duplicate OPPs for the CPUs.
        //
        // OPPs might be populated at runtime, don't fail for error here unless
        // it is -EPROBE_DEFER.
        let mut opp_table = match opp::Table::from_of_cpumask(dev, &mut mask) {
            Ok(table) => table,
            Err(e) => {
                if e == EPROBE_DEFER {
                    return Err(e);
                }

                // The table is added dynamically ?
                opp::Table::from_dev(dev)?
            }
        };

        // The OPP table must be initialized, statically or dynamically, by this point.
        opp_table.opp_count()?;

        // Set sharing cpus for fallback scenario.
        if fallback {
            mask.set_all();
            opp_table.set_sharing_cpus(&mut mask)?;
        }

        let mut transition_latency = opp_table.max_transition_latency() as u32;
        if transition_latency == 0 {
            transition_latency = cpufreq::ETERNAL_LATENCY;
        }

        let freq_table = opp_table.to_cpufreq_table()?;
        let clk = policy
            .set_freq_table(freq_table.table())
            .set_dvfs_possible_from_any_cpu()
            .set_suspend_freq((opp_table.suspend_freq() / 1000) as u32)
            .set_transition_latency(transition_latency)
            .set_clk(dev, None)?;

        mask.copy(policy.cpus());

        Ok(Arc::new(
            CPUFreqDTDevice {
                opp_table,
                freq_table,
                mask,
                token,
                clk,
            },
            GFP_KERNEL,
        )?)
    }

    fn exit(_policy: &mut cpufreq::Policy, _data: Option<Self::PData>) -> Result<()> {
        pr_info!("exit\n");
        Ok(())
    }

    fn online(_policy: &mut cpufreq::Policy) -> Result<()> {
        // We did light-weight tear down earlier, nothing to do here.
        pr_info!("online\n");
        Ok(())
    }

    fn offline(_policy: &mut cpufreq::Policy) -> Result<()> {
        // Preserve policy->data and don't free resources on light-weight
        // tear down.
        pr_info!("offline\n");
        Ok(())
    }

    fn suspend(policy: &mut cpufreq::Policy) -> Result<()> {
        policy.generic_suspend()
    }

    fn verify(data: &mut cpufreq::PolicyData) -> Result<()> {
        data.generic_verify()
    }

    fn target_index(policy: &mut cpufreq::Policy, index: u32) -> Result<()> {
        pr_info!("target_index\n");
        let data = match policy.data::<Self::PData>() {
            Some(data) => data,
            None => return Err(ENOENT),
        };

        let freq = data.freq_table.freq(index.try_into().unwrap())? as usize;
        data.opp_table.set_rate(freq * 1000)
    }

    fn get(policy: &mut cpufreq::Policy) -> Result<u32> {
        pr_info!("get\n");
        policy.generic_get()
    }

    fn set_boost(_policy: &mut cpufreq::Policy, _state: i32) -> Result<()> {
        pr_info!("set_boost\n");
        Ok(())
    }

    fn register_em(policy: &mut cpufreq::Policy) {
        policy.register_em_opp()
    }
}

kernel::of_device_table!(
    OF_TABLE,
    MODULE_OF_TABLE,
    <CPUFreqDTDriver as platform::Driver>::IdInfo,
    [
    (of::DeviceId::new(c_str!("operating-points-v2")), ())
    ]
);

impl platform::Driver for CPUFreqDTDriver {
    type IdInfo = ();
    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe(pdev: &mut platform::Device, _id_info: Option<&Self::IdInfo>) -> Result<Pin<KBox<Self>>> {
        cpufreq::Registration::<CPUFreqDTDriver>::new_foreign_owned(
            pdev.as_ref(),
            c_str!("cpufreq-dt"),
            (),
            cpufreq::flags::NEED_INITIAL_FREQ_CHECK | cpufreq::flags::IS_COOLING_DEV,
            true,
        )?;

        let drvdata = KBox::new(Self { _pdev: pdev.clone() }, GFP_KERNEL)?;

        Ok(drvdata.into())
    }
}

module_platform_driver! {
    type: CPUFreqDTDriver,
    name: "cpufreq_dt",
    author: "Viresh Kumar <viresh.kumar@linaro.org>",
    description: "Generic CPUFreq DT driver",
    license: "GPL v2",
}
