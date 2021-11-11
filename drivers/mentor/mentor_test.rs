// SPDX-License-Identifier: GPL-2.0

//! Mentor test

#![no_std]
#![feature(allocator_api, global_asm)]

use kernel::{mentor, prelude::*, str::CStr, ThisModule};

module! {
    type: MentorTest,
    name: b"mentor_test",
    author: b"Rust for Linux Contributors",
    description: b"Mentor Test",
    license: b"GPL v2",
    params: {
        write_addr: u8 {
            default: 0,
            permissions: 0,
            description: b"Address to write",
        },
        write_value: u32 {
            default: 42,
            permissions: 0,
            description: b"Value to write",
        },
    },
}

struct MentorTest;

impl KernelModule for MentorTest {
    fn init(_name: &'static CStr, _module: &'static ThisModule) -> Result<Self> {
        // Read module parameters
        let addr = *write_addr.read();
        let value = *write_value.read();

        // Never use bindings directly! Always create a safe abstraction.
        // This will likely be enforced in the future. Shown only
        // for demonstration purposes.
        {
            use kernel::bindings;

            pr_info!("--- Without an abstraction (do not use!)\n");

            pr_info!("Writing value {} to address {}\n", value, addr);
            unsafe { bindings::mentor_write(addr, value) };

            pr_info!("Reading from address {}\n", addr);
            let value = unsafe { bindings::mentor_read(addr) };
            pr_info!("Read value = {}\n", value);

            let total_writes =
                unsafe { bindings::mentor_read(bindings::MENTOR_TOTAL_WRITES_ADDR as u8) };
            pr_info!("Total writes = {}\n", total_writes);

            // We can produce undefined behavior, just like in C.
            let bad_addr = 0x42;
            pr_info!("Reading from address {}\n", bad_addr);
            let _ = unsafe { bindings::mentor_read(bad_addr) };
        }

        // The proper way.
        {
            pr_info!("--- With a safe abstraction\n");

            pr_info!("Writing value {} to address {}\n", value, addr);
            mentor::write(addr, value)?;

            pr_info!("Reading from address {}\n", addr);
            let value = mentor::read(addr)?;
            pr_info!("Read value = {}\n", value);

            let total_writes = mentor::read_total_writes();
            pr_info!("Total writes = {}\n", total_writes);

            // Whatever we try to do here, as long as it is safe code,
            // we cannot produce UB.
            let bad_addr = 0x42;
            pr_info!("Reading from address {}\n", bad_addr);
            if mentor::read(bad_addr).is_err() {
                pr_info!("Expected failure\n");
            }
        }

        Ok(MentorTest)
    }
}
