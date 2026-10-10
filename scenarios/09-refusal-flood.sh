#!/usr/bin/env bash
# Threat model B5 — refusal flooding of one context.
#
# Every refusal is a ledger event, and a context can be refused by anyone
# who can name it. So a chain records at most 32 refusals (DENIAL_CAP in
# ml-core): past that a refusal is still a refusal, exit 2 with its code,
# but the report says `recorded false` and the chain does not grow. The
# cap bounds refusals, not the purchase — the honest proof still pays —
# and the bounded chain still verifies.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario refusal-flood

cap=32
mandate=$(sign_mandate mandate)
authorize "$mandate" "$(cart cart "10.00")" "order-1"
expect_allowed "the purchase is authorized" state=authorized
ctx=$(field .context)

for n in $(seq 1 $((cap + 3))); do
    want=true
    [ "$n" -le "$cap" ] || want=false
    step pay "$ctx" --reference "$(ref "flood-$n")" --amount "10.00 INR" --invalid
    expect_exit 2 "invalid proof $n"
    expect_fields "invalid proof $n" refused=PROOF_INVALID "recorded=$want"
done
ok "$((cap + 3)) invalid proofs: every one refused, the first $cap recorded"

step log --context "$ctx"
[ "$(field '[.events[] | select(.event == "denied")] | length')" -eq "$cap" ] || fail "the chain holds exactly $cap refusals"
ok "the chain holds exactly $cap refusals"

step pay "$ctx" --reference "$(ref pay-ok)" --amount "10.00 INR"
expect_allowed "the honest proof still pays" state=paid
step settle "$ctx"
expect_allowed "and settles" state=settled
verify_evidence "$ctx"
