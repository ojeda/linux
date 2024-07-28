#!/bin/sh
# SPDX-License-Identifier: GPL-2.0
#
# Usage: $ ./pahole-version.sh pahole
#
# Prints pahole's version in a 3-digit form, such as 119 for v1.19.

set -e
trap "echo 0; exit 1" EXIT

if ! command -v "$@" >/dev/null; then
	echo >&2 "***"
	echo >&2 "*** pahole '$@' could not be found. pahole will not be used."
	echo >&2 "***"
	exit 1
fi

output=$("$@" --version 2>/dev/null) || code=$?
if [ -n "$code" ]; then
	echo >&2 "***"
	echo >&2 "*** Running '$@' to check the pahole version failed with"
	echo >&2 "*** code $code. pahole will not be used."
	echo >&2 "***"
	exit 1
fi

output=$(echo "$output" | sed -nE 's/v([0-9]+)\.([0-9]+)/\1\2/p')
if [ -z "${output}" ]; then
	echo >&2 "***"
	echo >&2 "*** pahole '$@' returned an unexpected version output."
	echo >&2 "*** pahole will not be used."
	echo >&2 "***"
	exit 1
fi

echo "${output}"
trap EXIT
