// SPDX-License-Identifier: GPL-2.0

//! Rust minimal sample.

use kernel::prelude::*;

module! {
    type: RustMinimal,
    name: "rust_minimal",
    author: "Rust for Linux Contributors",
    description: "Rust minimal sample",
    license: "GPL",
}

use kernel::macros::lazy_static;
lazy_static! {
    static ref TEST: u32 = {
        pr_info!("Testing lazy_static...\n");
        42
    };
}

struct RustMinimal {
    message: String,
}

impl kernel::Module for RustMinimal {
    fn init(_name: &'static CStr, _module: &'static ThisModule) -> Result<Self> {
        pr_info!("Rust minimal sample (init)\n");
        pr_info!("Am I built-in? {}\n", !cfg!(MODULE));

        pr_info!("1 {}\n", *TEST);
        pr_info!("2 {}\n", *TEST);
        pr_info!("3 {}\n", *TEST);

        Ok(RustMinimal {
            message: "on the heap!".try_to_owned()?,
        })
    }
}

impl Drop for RustMinimal {
    fn drop(&mut self) {
        pr_info!("My message is {}\n", self.message);
        pr_info!("Rust minimal sample (exit)\n");
    }
}
