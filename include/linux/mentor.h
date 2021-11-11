/* SPDX-License-Identifier: GPL-2.0 */
/*
 * The example mentor subsystem: a key-value "database".
 *
 * Valid addresses go from 0x00 to 0x05. Accessing others is UB.
 *
 * Reading address 0x05 gives the total number of writes.
 * Writing to it is UB.
 */
#ifndef __LINUX_MENTOR_H
#define __LINUX_MENTOR_H

#include <linux/compiler.h>

#define MENTOR_TOTAL_WRITES_ADDR 0x05

/* Public interface */
#define mentor_read(addr) \
	__mentor_read(addr)
void mentor_write(u8 addr, u32 value);

/* Do not use! */
u32 __mentor_read(u8 addr);

#endif /* __LINUX_MENTOR_H */
