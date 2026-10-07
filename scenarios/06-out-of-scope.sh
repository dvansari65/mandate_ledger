#!/usr/bin/env bash
# Threat model A7, A9 — out-of-scope purchase (P14), unsigned terms (P18).
#
# `cart.claims ⊆ mandate.scope`, or nothing is reserved. A cart at another
# merchant, in another category, over the per-purchase cap, in another
# currency, without a category the mandate constrains, or without the
# merchant's signature the mandate requires: each is refused with its own
# code, in a chain of its own. A cart signed under a key the operator does
# not know never reaches the ledger at all. Then a cart inside the scope is
# authorized, so the scope admits something.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario out-of-scope

mandate=$(sign_mandate mandate)

# Authorize this cart and expect this refusal, as the report and as the
# whole chain. Leaves the refused context in `ctx`.
refuse() { # cart-file CODE description
    authorize "$mandate" "$1" "order-$2"
    expect_refused "$2" true "$3"
    ctx=$(field .context)
    expect_chain "$ctx" "denied:$2"
}

refuse "$(cart merchant "100.00" '.merchant = "amazon.in"')" SCOPE_MERCHANT_MISMATCH "a merchant outside the scope"
refuse "$(cart category "100.00" '.category = "toys"')" SCOPE_CATEGORY_MISMATCH "a category outside the scope"
refuse "$(cart cap "2000.01")" SCOPE_PER_TXN_EXCEEDED "a paisa over the per-purchase cap"
refuse "$(cart currency "100.00" '.total.currency = "USD"')" CURRENCY_MISMATCH "another currency"
refuse "$(cart uncategorized "100.00" 'del(.category)')" UNVERIFIABLE_SCOPE "no category where the mandate constrains one"
refuse "$(raw_cart unsigned "100.00")" ATTESTATION_INSUFFICIENT "a cart the merchant did not sign"

# Signed under a key id the operator never named: the adapter rejects it
# before the ledger sees it. An error, not a decision; nothing is recorded.
step authorize --mandate "$mandate" --cart "$(cart stranger "100.00")" --request-key order-stranger \
    --trust "$PRINCIPAL=$USER_KEY.pub"
expect_undecided "unknown merchant key" "a cart signed under an unknown key never reaches the ledger"

expect_contexts authorized 0 "six refusals, nothing reserved"

authorize "$mandate" "$(cart inside "2000.00" '.merchant = "quick.zepto.com"')" "order-inside"
expect_allowed "a cart inside the scope, at the cap, at a wildcard merchant" state=authorized merchant=quick.zepto.com
inside=$(field .context)

verify_evidence "$inside"
verify_evidence "$ctx"
