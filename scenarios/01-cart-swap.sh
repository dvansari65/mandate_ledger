#!/usr/bin/env bash
# Threat model A5 — cart swap (P1, P2).
#
# The cart approved is the cart paid. A proof bound to another cart's hash,
# a proof for another amount, and a proof bound to nothing are each refused
# and recorded; the proof for the approved cart then pays, settles and
# delivers, and the chain carries all of it.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario cart-swap

mandate=$(sign_mandate mandate)
approved=$(cart approved "128.00")
cart swapped "1999.00" >/dev/null # only its hash is presented

authorize "$mandate" "$approved" "order-1"
expect_allowed "the agent's cart is authorized" state=authorized amount="128.00 INR" cart_hash="$(cart_hash approved)"
ctx=$(field .context)

step pay "$ctx" --reference "$(ref pay-ok)" --amount "128.00 INR" --bound-cart "$(cart_hash swapped)"
expect_refused CART_BINDING_MISMATCH true "a proof bound to the swapped cart"

step pay "$ctx" --reference "$(ref pay-ok)" --amount "1999.00 INR"
expect_refused AMOUNT_MISMATCH true "a proof for the swapped cart's amount"
expect_detail "authorized 128.00 INR" "the refusal names the authorized amount"

step pay "$ctx" --reference "$(ref pay-ok)" --amount "128.00 INR" --unbound
expect_refused UNBOUND_PROOF true "a proof bound to neither context nor cart"

step pay "$ctx" --reference "$(ref pay-ok)" --amount "128.00 INR" --bound-cart "$(cart_hash approved)"
expect_allowed "the proof for the approved cart pays" state=paid
step settle "$ctx"
expect_allowed "the rail calls it final" state=settled
step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_allowed "the merchant delivers" state=delivered

expect_chain "$ctx" "authorized denied:CART_BINDING_MISMATCH denied:AMOUNT_MISMATCH denied:UNBOUND_PROOF paid settled delivered"
verify_evidence "$ctx"
