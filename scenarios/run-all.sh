#!/usr/bin/env bash
# Run every scenario, in order, and report; exit 1 if any failed. A failed
# scenario does not stop the rest. They run one after another, not at
# once: the sandbox clock is global to the database, and the time-window
# scenario freezes it.
#
#   ML_DATABASE_URL=postgres://localhost/mandate_ledger scenarios/run-all.sh
#
# `ML` names the binary to drive (default: build `ml` from this checkout);
# `KEEP=1` keeps every scenario's working directory.

dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$dir/lib.sh" # checks the prerequisites and resolves ML, once
export ML

passed=0
failed=
for scenario in "$dir"/[0-9][0-9]-*.sh; do
    if bash "$scenario"; then
        passed=$((passed + 1))
    else
        failed="$failed ${scenario##*/}"
    fi
    echo
done

if [ -n "$failed" ]; then
    echo "$passed passed; failed:$failed" >&2
    exit 1
fi
echo "all $passed scenarios passed"
