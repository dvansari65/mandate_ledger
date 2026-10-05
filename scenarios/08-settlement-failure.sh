#!/usr/bin/env bash
# Threat model A4, A10 — no rollback after failed settlement (P6), finality
# from a rail that did not take the payment (P18).
#
# `Paid → SettlementFailed → Compensated` is a first-class path. A payment
# dropped in a reorganization on the check that would have made it final,
# and one the rail declined outright, both end in compensation; neither
# can be delivered; a failed settlement releases its reservation, so the
# budget it held is spendable again. Finality claimed by another rail is
# refused and recorded. The rail's retry of a finality report answers as
# the first did and adds nothing.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario settlement-failure

# A budget of 1,000: one 600 reservation leaves no room for another unless
# the first is released.
mandate=$(sign_mandate mandate 'del(.scope.velocity) | .scope.max_total.amount = "1000.00"')

# ── Reorganized, at a finality of three confirmations ────────────────────

authorize "$mandate" "$(cart reorg "600.00")" "order-reorg"
expect_allowed "600 of the 1,000 budget is reserved" state=authorized
reorg=$(field .context)
step pay "$reorg" --reference "$(ref pay-reorg)" --amount "600.00 INR"
expect_allowed "the payment is recorded" state=paid

step settle "$reorg" --confirmations 3 --rail other
expect_refused RAIL_MISMATCH true "finality claimed by a rail that did not take the payment"

for have in 1 2; do
    step settle "$reorg" --finality 3
    expect_allowed "confirmation $have of 3" state=pending reason="need 3 confirmations, have $have"
done
step settle "$reorg" --finality 3
expect_allowed "dropped in a reorganization on the check that would have made it final" \
    state=settlement_failed reason="reorganized: dropped after 2 confirmations"

step deliver "$reorg" --receipt BB-1 --signed-by bb
expect_refused INVALID_STATE false "delivery after a failed settlement"

# The reservation is released: another 600 fits in the 1,000.
authorize "$mandate" "$(cart next "600.00")" "order-next"
expect_allowed "the failed payment's 600 is spendable again" state=authorized

step compensate "$reorg" --reference "$(ref refund)"
expect_allowed "the host records its compensation" state=compensated
step settle "$reorg" --finality 3
expect_allowed "the rail's retried report answers as before" state=settlement_failed
expect_chain "$reorg" "authorized paid denied:RAIL_MISMATCH settlement_failed compensated"

# ── Declined outright ────────────────────────────────────────────────────

authorize "$mandate" "$(cart declined "400.00")" "order-declined"
expect_allowed "a third purchase fills the budget" state=authorized
declined=$(field .context)
step pay "$declined" --reference "$(ref pay-fail)" --amount "400.00 INR"
expect_allowed "the payment is recorded" state=paid
step settle "$declined"
expect_allowed "the rail declines it" state=settlement_failed reason="declined by the rail"
step deliver "$declined" --receipt BB-2 --signed-by bb
expect_refused INVALID_STATE false "delivery after a declined payment"
step compensate "$declined" --reference "$(ref reversal)"
expect_allowed "the host records its compensation" state=compensated
expect_chain "$declined" "authorized paid settlement_failed compensated"

expect_contexts compensated 2 "both failures are compensated"
verify_evidence "$reorg"
verify_evidence "$declined"
