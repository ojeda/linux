// SPDX-License-Identifier: GPL-2.0

// Copyright (C) 2024 Google LLC.

//! X86 Rust implementation of jump_label.h

/// x86 implementation of arch_static_branch
#[macro_export]
#[cfg(target_arch = "x86_64")]
macro_rules! arch_static_branch {
    ($key:path, $keytyp:ty, $field:ident, $branch:expr) => {'my_label: {
        core::arch::asm!(
            include!(concat!(env!("SRCTREE"), "/rust/kernel/arch_static_branch_asm.rs")),
            label {
                break 'my_label true;
            },
            sym $key,
            const ::core::mem::offset_of!($keytyp, $field),
            const $crate::arch::bool_to_int($branch),
        );

        break 'my_label false;
    }};
}

pub use arch_static_branch;
