#!/usr/bin/env bash
# Threat model A1, A2 — double-charge on retry, nonce race (P3); A14 —
# a retry under a forged mandate (P7, P15).
#
# A retry is the same purchase attempt, a forgery is not: the same request
# key under a mandate with the same id but another signer lands on the
# same context and is refused there. One payment nonce, one payment: the
# same proof retried against the same context is the same payment, and the
# engine answers as it did and adds nothing to the chain; that nonce
# presented for a second context, under any reference, is refused and
# recorded against the second context, which then pays with a nonce of its
# own.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario replayed-proof

mandate=$(sign_mandate mandate)
cart=$(cart cart "10.00")
nonce=$(ref nonce-ok)

authorize "$mandate" "$cart" "order-a"
expect_allowed "context A is authorized" state=authorized
a=$(field .context)

# The same request key under the same mandate body signed by a key of the
# attacker's own: the same context, not its token.
"$ML" keys new --out "$WORK/attacker.key" >/dev/null
"$ML" mandate sign "$WORK/mandate.body.json" --key "$WORK/attacker.key" --out "$WORK/forged.json" >/dev/null
authorize "$WORK/forged.json" "$cart" "order-a"
expect_fields "A's request key under a forged mandate" "context=$a"
expect_refused CONTEXT_MISMATCH true "A's request key under a forged mandate lands on A"
authorize "$mandate" "$cart" "order-a"
expect_allowed "the same mandate again is the same purchase attempt" context="$a"
authorize "$mandate" "$cart" "order-b"
expect_allowed "context B is authorized: same cart, its own request key" state=authorized
b=$(field .context)

step pay "$a" --reference "$nonce" --amount "10.00 INR"
expect_allowed "A is paid" state=paid
key=$(field .idempotency_key)
step pay "$a" --reference "$nonce" --amount "10.00 INR"
expect_allowed "the same proof again is the same payment" state=paid idempotency_key="$key"
expect_chain "$a" "authorized denied:CONTEXT_MISMATCH paid"

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
