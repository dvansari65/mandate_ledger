#!/usr/bin/env bash
# Threat model A6 — budget overrun under concurrency (P8).
#
# Sixteen processes, started together, each try to authorize 1,000 INR
# under a mandate capped at 8,000 in all. The reservation is applied inside
# the store's append, in one transaction under a lock on the mandate, so
# exactly eight may pass whatever the interleaving. Each process is a real
# `ml authorize` with its own connection; the only thing they share is the
# database.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario budget-race

# No velocity limit: the budget is the only thing that may refuse.
mandate=$(sign_mandate mandate 'del(.scope.velocity)')
cart=$(cart cart "1000.00")
race="$WORK/race"
mkdir "$race"

for i in $(seq 1 16); do
    (
        set +e
        "$ML" --json authorize --mandate "$mandate" --cart "$cart" --request-key "race-$i" \
            "${TRUST[@]}" >"$race/$i.out" 2>"$race/$i.err"
        echo $? >"$race/$i.code"
    ) &
done
wait

allowed=0
refused=0
for i in $(seq 1 16); do
    recall "$race/$i" "process $i"
    case "$CODE" in
    0)
        expect_fields "process $i" state=authorized
        allowed=$((allowed + 1))
        allowed_ctx=$(field .context)
        ;;
    2)
        expect_fields "process $i" refused=SCOPE_TOTAL_EXCEEDED recorded=true
        refused=$((refused + 1))
        refused_ctx=$(field .context)
        ;;
    *) fail "process $i could not decide" ;;
    esac
done
[ "$allowed" -eq 8 ] || fail "$allowed were allowed, expected exactly 8"
[ "$refused" -eq 8 ] || fail "$refused were refused, expected exactly 8"
ok "16 processes at once: 8 allowed, 8 refused SCOPE_TOTAL_EXCEEDED, none undecided"
expect_contexts authorized 8 "the mandate holds exactly 8 authorized contexts"

# The budget is spent to the paisa: one more rupee is refused, and the
# refusal says what is left.
authorize "$mandate" "$(cart one "1.00")" "after-the-race"
expect_refused SCOPE_TOTAL_EXCEEDED true "one more rupee after the race"
expect_detail "remaining budget 0.00 INR" "nothing remains"

expect_chain "$refused_ctx" "denied:SCOPE_TOTAL_EXCEEDED"
verify_evidence "$allowed_ctx"
verify_evidence "$refused_ctx"
