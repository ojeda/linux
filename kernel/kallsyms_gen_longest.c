// SPDX-License-Identifier: GPL-2.0-only
/*
 * kallsyms_gen_longest - Generate the longest symbol possible.
 */

#include <stddef.h>
#include <stdio.h>

#include <linux/kallsyms.h>

int main(void)
{
        const char start[] = "start_of_the_longest_symbol_possible__";
        const char end[] = "__end_of_the_longest_symbol_possible";
        const char pattern[] = "123456789_";

        _Static_assert((KSYM_NAME_LEN - 1) >= (sizeof(start) - 1) + (sizeof(end) - 1), "");

        printf(start);

        const size_t pattern_length = (KSYM_NAME_LEN - 1) - (sizeof(start) - 1) - (sizeof(end) - 1);
        for (size_t i = 0; i < pattern_length; ++i)
                putchar(pattern[i % (sizeof(pattern) - 1)]);

        printf(end);

        return 0;
}
