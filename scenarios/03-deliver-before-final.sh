#!/usr/bin/env bash
# Threat model A3 — release before finality (P5).
#
# Delivery needs a `Settled` token and nothing else. Before payment, while
# the rail still calls the payment pending, and with evidence short of the
# rail's finality threshold, the merchant cannot deliver: the type system
# says no before the engine can be asked, so there is a report with the
# engine's code and nothing in the chain. Delivery lands once, after
# finality, and the chain is the clean lifecycle.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario deliver-before-final

mandate=$(sign_mandate mandate)
cart=$(cart cart "640.00")

authorize "$mandate" "$cart" "order-1"
expect_allowed "the cart is authorized" state=authorized
ctx=$(field .context)

step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_refused INVALID_STATE false "delivery before payment"
expect_detail "the context is authorized" "the report says where the context stands"

step settle "$ctx" --confirmations 1
expect_refused INVALID_STATE false "settlement before payment"
expect_detail "never paid" "the report says why"

step pay "$ctx" --reference "$(ref pay)" --amount "640.00 INR"
expect_allowed "the payment is recorded" state=paid

step settle "$ctx" --confirmations 0
expect_allowed "the rail has not confirmed it" state=pending

step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_refused INVALID_STATE false "delivery while the payment is pending"
expect_detail "the context is paid" "the report says where the context stands"

step settle "$ctx" --confirmations 2 --finality 3
expect_allowed "two confirmations are short of a finality of three" state=pending reason="need 3 confirmations, have 2"

step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_refused INVALID_STATE false "delivery short of the finality threshold"

expect_chain "$ctx" "authorized paid"

step settle "$ctx" --confirmations 3 --finality 3
expect_allowed "three confirmations are final" state=settled reference="$(ref pay)@3"

step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_allowed "delivery after finality" state=delivered receipt=BB-1

expect_chain "$ctx" "authorized paid settled delivered"
verify_evidence "$ctx"
