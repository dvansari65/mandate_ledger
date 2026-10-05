#!/usr/bin/env bash
# Threat model A1, A2 — double-charge on retry, nonce race (P3).
#
# One payment nonce, one payment. The same proof retried against the same
# context is the same payment: the engine answers as it did and adds
# nothing to the chain. That nonce presented for a second context, under
# any reference, is refused and recorded against the second context, which
# then pays with a nonce of its own.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario replayed-proof

mandate=$(sign_mandate mandate)
cart=$(cart cart "10.00")
nonce=$(ref nonce-ok)

authorize "$mandate" "$cart" "order-a"
expect_allowed "context A is authorized" state=authorized
a=$(field .context)
authorize "$mandate" "$cart" "order-b"
expect_allowed "context B is authorized: same cart, its own request key" state=authorized
b=$(field .context)

step pay "$a" --reference "$nonce" --amount "10.00 INR"
expect_allowed "A is paid" state=paid
key=$(field .idempotency_key)
step pay "$a" --reference "$nonce" --amount "10.00 INR"
expect_allowed "the same proof again is the same payment" state=paid idempotency_key="$key"
expect_chain "$a" "authorized paid"

step pay "$b" --reference "$(ref other-ok)" --amount "10.00 INR" --nonce "$nonce"
expect_refused NONCE_ALREADY_USED true "A's nonce under another reference, for B"
step pay "$b" --reference "$nonce" --amount "10.00 INR"
expect_refused NONCE_ALREADY_USED true "A's proof itself, for B"
step pay "$b" --reference "$(ref own-ok)" --amount "10.00 INR"
expect_allowed "B pays with its own nonce" state=paid

expect_chain "$b" "authorized denied:NONCE_ALREADY_USED denied:NONCE_ALREADY_USED paid"
expect_contexts paid 2 "two contexts, two payments, two nonces"
verify_evidence "$a"
verify_evidence "$b"
