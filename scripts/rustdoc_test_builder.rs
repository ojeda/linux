// SPDX-License-Identifier: GPL-2.0

//! Generates KUnit tests from `rustdoc`-generated doctests.
//!
//! KUnit passes a context (`struct kunit *`) to each test, which should be forwarded to the other
//! KUnit functions and macros.
//!
//! However, we want to keep this as an implementation detail because:
//!
//!   - Test code should not care about the implementation.
//!
//!   - Documentation looks worse if it needs to carry extra details unrelated to the piece
//!     being described.
//!
//!   - Test code should be able to define functions and call them, without having to carry
//!     the context.
//!
//!   - Later on, we may want to be able to test non-kernel code (e.g. `core` or third-party
//!     crates) which likely use the standard library `assert*!` macros.
//!
//! For this reason, instead of the passed context, `kunit_get_current_test()` is used instead
//! (i.e. `current->kunit_test`).
//!
//! Note that this means other threads/tasks potentially spawned by a given test, if failing, will
//! report the failure in the kernel log but will not fail the actual test. Saving the pointer in
//! e.g. a `static` per test does not fully solve the issue either, because currently KUnit does
//! not support assertions (only expectations) from other tasks. Thus leave that feature for
//! the future, which simplifies the code here too. We could also simply not allow `assert`s in
//! other tasks, but that seems overly constraining, and we do want to support them, eventually.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
};

use json::JsonValue;

mod json;

fn main() {
    let mut stdin = std::io::stdin().lock();
    let mut rustdoc_json = String::new();
    stdin.read_to_string(&mut rustdoc_json).unwrap();

    let JsonValue::Object(rustdoc) = JsonValue::parse(&rustdoc_json).unwrap() else {
        panic!("Expected an object")
    };

    let Some(JsonValue::Number(format_version)) = rustdoc.get("format_version") else {
        panic!("missing `format_version` field");
    };
    assert!(
        *format_version == 2,
        "unsupported rustdoc format version: {format_version}"
    );

    let Some(JsonValue::Array(doctests)) = rustdoc.get("doctests") else {
        panic!("`doctests` field is missing or has the wrong type");
    };

    let mut nb_generated = 0;
    let mut number = 0;
    let mut last_file = "";
    let mut rust_tests = String::new();
    let mut c_test_declarations = String::new();
    let mut c_test_cases = String::new();
    for doctest in doctests {
        let JsonValue::Object(doctest) = doctest else {
            unreachable!()
        };

        // We check if we need to skip this test by checking it's a rust code and it's not ignored.
        if let Some(JsonValue::Object(attributes)) = doctest.get("doctest_attributes") {
            if attributes.get("rust") != Some(&JsonValue::Bool(true)) {
                continue;
            }
            if let Some(JsonValue::String(ignore)) = attributes.get("ignore") {
                if ignore != "None" {
                    continue;
                }
            }
        }

        let (
            Some(JsonValue::String(file)),
            Some(JsonValue::Number(line)),
            Some(JsonValue::String(name)),
            Some(JsonValue::Object(doctest_code)),
        ) = (
            doctest.get("file"),
            doctest.get("line"),
            doctest.get("name"),
            doctest.get("doctest_code"),
        )
        else {
            continue;
        };

        let (
            Some(JsonValue::String(code)),
            Some(JsonValue::String(crate_level_code)),
            Some(JsonValue::Object(wrapper)),
        ) = (
            doctest_code.get("code"),
            doctest_code.get("crate_level"),
            doctest_code.get("wrapper"),
        )
        else {
            continue;
        };

        let (Some(JsonValue::String(before)), Some(JsonValue::String(after))) =
            (wrapper.get("before"), wrapper.get("after"))
        else {
            continue;
        };

        // For tests that get generated with `Result`, `rustdoc` generates an `unwrap()` on
        // the return value to check there were no returned errors. Instead, we use our assert macro
        // since we want to just fail the test, not panic the kernel.
        //
        // We save the result in a variable so that the failed assertion message looks nicer.
        let after = if let Some(JsonValue::Bool(true)) = wrapper.get("returns_result") {
            "\n} let test_return_value = _inner(); assert!(test_return_value.is_ok()); }"
        } else {
            after.as_str()
        };

        let body = format!("{crate_level_code}\n{before}\n{code}{after}\n");
        nb_generated += 1;

        // Generate an ID sequence ("test number") for each one in the file.
        if file == last_file {
            number += 1;
        } else {
            number = 0;
            last_file = file;
        }

        // Generate a KUnit name (i.e. test name and C symbol) for this test.
        //
        // We avoid the line number, like `rustdoc` does, to make things slightly more stable for
        // bisection purposes. However, to aid developers in mapping back what test failed, we will
        // print a diagnostics line in the KTAP report.
        let kunit_name = format!(
            "rust_doctest_{}_{number}",
            file.replace('/', "_").replace('.', "_")
        );

        // Calculate how many lines before `main` function (including the `main` function line).
        let body_offset = body
            .lines()
            .take_while(|line| !line.contains("fn main() {"))
            .count()
            + 1;

        use std::fmt::Write;
        write!(
            rust_tests,
            r#"/// Generated `{name}` KUnit test case from a Rust documentation test.
#[no_mangle]
pub extern "C" fn {kunit_name}(__kunit_test: *mut ::kernel::bindings::kunit) {{
    /// Overrides the usual [`assert!`] macro with one that calls KUnit instead.
    #[allow(unused)]
    macro_rules! assert {{
        ($cond:expr $(,)?) => {{{{
            ::kernel::kunit_assert!(
                "{kunit_name}", "{file}", __DOCTEST_ANCHOR - {line}, $cond
            );
        }}}}
    }}

    /// Overrides the usual [`assert_eq!`] macro with one that calls KUnit instead.
    #[allow(unused)]
    macro_rules! assert_eq {{
        ($left:expr, $right:expr $(,)?) => {{{{
            ::kernel::kunit_assert_eq!(
                "{kunit_name}", "{file}", __DOCTEST_ANCHOR - {line}, $left, $right
            );
        }}}}
    }}

    // Many tests need the prelude, so provide it by default.
    #[allow(unused)]
    use ::kernel::prelude::*;

    // Unconditionally print the location of the original doctest (i.e. rather than the location in
    // the generated file) so that developers can easily map the test back to the source code.
    //
    // This information is also printed when assertions fail, but this helps in the successful cases
    // when the user is running KUnit manually, or when passing `--raw_output` to `kunit.py`.
    //
    // This follows the syntax for declaring test metadata in the proposed KTAP v2 spec, which may
    // be used for the proposed KUnit test attributes API. Thus hopefully this will make migration
    // easier later on.
    ::kernel::kunit::info(fmt!("    # {kunit_name}.location: {file}:{line}\n"));

    /// The anchor where the test code body starts.
    #[allow(unused)]
    static __DOCTEST_ANCHOR: i32 = ::core::line!() as i32 + {body_offset} + 1;
    {{
        {body}
        main();
    }}
}}

"#
        )
        .unwrap();

        write!(c_test_declarations, "void {kunit_name}(struct kunit *);\n").unwrap();
        write!(c_test_cases, "    KUNIT_CASE({kunit_name}),\n").unwrap();
    }

    if nb_generated == 0 {
        panic!("No test function found in `rustdoc`'s output.");
    }

    let rust_tests = rust_tests.trim();
    let c_test_declarations = c_test_declarations.trim();
    let c_test_cases = c_test_cases.trim();

    write!(
        BufWriter::new(File::create("rust/doctests_kernel_generated.rs").unwrap()),
        r#"//! `kernel` crate documentation tests.

const __LOG_PREFIX: &[u8] = b"rust_doctests_kernel\0";

{rust_tests}
"#
    )
    .unwrap();

    write!(
        BufWriter::new(File::create("rust/doctests_kernel_generated_kunit.c").unwrap()),
        r#"/*
 * `kernel` crate documentation tests.
 */

#include <kunit/test.h>

{c_test_declarations}

static struct kunit_case test_cases[] = {{
    {c_test_cases}
    {{ }}
}};

static struct kunit_suite test_suite = {{
    .name = "rust_doctests_kernel",
    .test_cases = test_cases,
}};

kunit_test_suite(test_suite);

MODULE_LICENSE("GPL");
"#
    )
    .unwrap();
}
