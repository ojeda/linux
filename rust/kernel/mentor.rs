// SPDX-License-Identifier: GPL-2.0

//! Mentor subsystem.
//!
//! C headers: [`include/linux/mentor.h`](../../../../include/linux/mentor.h)

use crate::{bindings, error::Error, Result};

const TOTAL_WRITES_ADDR: u8 = bindings::MENTOR_TOTAL_WRITES_ADDR as u8;

fn is_valid(addr: u8) -> bool {
    addr < TOTAL_WRITES_ADDR
}

/// Reads from an address (unchecked version).
///
/// To read the total number of writes, use [`read_total_writes`] instead.
///
/// # Safety
///
/// The address must be valid.
///
/// # Examples
///
/// ```
/// # use kernel::prelude::*;
/// # use kernel::mentor;
/// # fn test() {
/// let result = unsafe { mentor::read_unchecked(0x01) };
/// # }
/// ```
pub unsafe fn read_unchecked(addr: u8) -> u32 {
    // SAFETY: FFI call, the caller guarantees the address is valid.
    unsafe { bindings::mentor_read(addr) }
}

/// Reads from an address.
///
/// To read the total number of writes, use [`read_total_writes`] instead.
///
/// Returns an error if the address is invalid.
///
/// # Examples
///
/// ```
/// # use kernel::prelude::*;
/// # use kernel::mentor;
/// # fn test() -> Result {
/// let result = mentor::read(0x01)?;
/// # Ok(())
/// # }
/// ```
pub fn read(addr: u8) -> Result<u32> {
    if !is_valid(addr) {
        return Err(Error::EINVAL);
    }

    // SAFETY: FFI call, we have verified the address is valid.
    Ok(unsafe { bindings::mentor_read(addr) })
}

/// Writes a value to an address (unchecked version).
///
/// # Safety
///
/// The address must be valid.
///
/// # Examples
///
/// ```
/// # use kernel::prelude::*;
/// # use kernel::mentor;
/// # fn test() {
/// unsafe { mentor::write_unchecked(0x01, 42); }
/// # }
/// ```
pub unsafe fn write_unchecked(addr: u8, value: u32) {
    // SAFETY: FFI call, the caller guarantees the address is valid.
    unsafe { bindings::mentor_write(addr, value) }
}

/// Writes a value to an address.
///
/// Returns an error if the address is invalid.
///
/// # Examples
///
/// ```
/// # use kernel::prelude::*;
/// # use kernel::mentor;
/// # fn test() -> Result {
/// mentor::write(0x01, 42)?;
/// # Ok(())
/// # }
/// ```
pub fn write(addr: u8, value: u32) -> Result {
    if !is_valid(addr) {
        return Err(Error::EINVAL);
    }

    // SAFETY: FFI call, we have verified the address is valid.
    unsafe { bindings::mentor_write(addr, value) }

    Ok(())
}

/// Reads the total number of writes (from the special Mentor address).
///
/// # Examples
///
/// ```
/// # use kernel::prelude::*;
/// # use kernel::mentor;
/// # fn test() {
/// let total_writes = mentor::read_total_writes();
/// # }
/// ```
pub fn read_total_writes() -> u32 {
    // SAFETY: FFI call, this address is always valid.
    unsafe { bindings::mentor_read(TOTAL_WRITES_ADDR) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid() {
        assert!(is_valid(0x00));
        assert!(is_valid(0x04));
        assert!(!is_valid(0x05));
    }
}
