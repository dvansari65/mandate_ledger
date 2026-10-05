#!/usr/bin/env bash
# Threat model A8 — revoked mandate (P17).
#
# Once a mandate is revoked, every later authorization under it is refused
# and recorded; the refusal is decided inside the store's own transaction,
# so it cannot race the revocation. A context authorized before the
# revocation is unaffected, and a retry of that purchase attempt is still
# that purchase, not a new one.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario revoked-mandate

mandate=$(sign_mandate mandate)
before=$(cart before "300.00")
after=$(cart after "300.00")

authorize "$mandate" "$before" "order-1"
expect_allowed "a purchase is authorized before the revocation" state=authorized
ctx=$(field .context)

step revoke "$MANDATE_ID"
expect_allowed "the principal revokes the mandate" revoked=true

authorize "$mandate" "$after" "order-2"
expect_refused MANDATE_REVOKED true "a new purchase under the revoked mandate"
refused=$(field .context)
authorize "$mandate" "$after" "order-2"
expect_refused MANDATE_REVOKED true "the same purchase tried again"

authorize "$mandate" "$before" "order-1"
expect_allowed "the earlier purchase attempt is still that purchase" state=authorized context="$ctx"
step pay "$ctx" --reference "$(ref pay-ok)" --amount "300.00 INR"
expect_allowed "the earlier purchase is paid" state=paid
step settle "$ctx"
expect_allowed "and settled" state=settled
step deliver "$ctx" --receipt BB-1 --signed-by bb
expect_allowed "and delivered" state=delivered

expect_chain "$ctx" "authorized paid settled delivered"
expect_chain "$refused" "denied:MANDATE_REVOKED denied:MANDATE_REVOKED"
expect_contexts authorized 0 "the refused purchase reserved nothing"
verify_evidence "$ctx"
verify_evidence "$refused"
