# Scenarios

Nine scripts, one per row of [the threat model](../docs/threat-model.md),
each driving the engine the way an attacker and then an honest party would:
through `ml`, one process per step, against PostgreSQL. Every script
asserts the exit code of every step, the refusal code in the report and in
the context's chain, and that the evidence `ml evidence` exports verifies
with `ml verify` on a machine with no database — and that an altered copy
does not. CI runs them on every push against a PostgreSQL service.

```bash
ML_DATABASE_URL=postgres://localhost/mandate_ledger scenarios/run-all.sh
ML_DATABASE_URL=postgres://localhost/mandate_ledger scenarios/05-budget-race.sh
```

Needs `jq`, and either `ML` naming an `ml` binary or a cargo toolchain to
build one. The scripts run one after another, not at once: the sandbox
clock is global to the database, and the time-window scenario freezes it
(and puts it back, however the scenario ends). Run them against a database
nothing else is using.

| Script | Row | What is driven, and what must happen |
|---|---|---|
| `01-cart-swap` | A5, A10 | A proof bound to another cart's hash, one for another amount, one bound to nothing, one paying someone else: `CART_BINDING_MISMATCH`, `AMOUNT_MISMATCH`, `UNBOUND_PROOF`, `MERCHANT_BINDING_MISMATCH`, each recorded. The proof for the approved cart, naming the approved merchant, pays, settles, delivers. |
| `02-replayed-proof` | A1, A2, A14 | The same request key under a forged mandate with the same id lands on the same context: `CONTEXT_MISMATCH`, recorded there; the honest retry still answers. The same proof retried is the same payment and adds nothing to the chain. Its nonce presented for a second context, under any reference: `NONCE_ALREADY_USED`, recorded against that context. |
| `03-deliver-before-final` | A3 | Delivery before payment, while pending, and short of the finality threshold: `INVALID_STATE` with nothing recorded, because the type system refused before the engine could be asked. The chain stays `authorized paid` until the rail calls it final. |
| `04-revoked-mandate` | A8 | After `ml revoke`, a new purchase is `MANDATE_REVOKED`, recorded, and reserves nothing. The purchase authorized before it continues to delivery, and its retry is still that purchase. |
| `05-budget-race` | A6 | Sixteen `ml authorize` processes started together, 1,000 each under an 8,000 budget: exactly eight allowed, eight `SCOPE_TOTAL_EXCEEDED`, none undecided; then one rupee more is refused with `remaining budget 0.00 INR`. |
| `06-out-of-scope` | A7, A9 | Merchant, category, per-purchase cap, currency, a missing category, an unsigned cart: six codes, six chains of one event, nothing reserved. A cart signed under an unknown key is exit 1, not a decision. A cart inside the scope is authorized. |
| `07-time-window` | A8 | With the clock frozen: `MANDATE_NOT_YET_VALID` before the window, `MANDATE_EXPIRED` after it, `VELOCITY_EXCEEDED` on the fourth purchase in an hour, allowed once the hour has passed. Every event carries the frozen instant. |
| `08-settlement-failure` | A4, A10 | A payment dropped in a reorganization on the check that would have made it final, and one the rail declined: both `settlement_failed`, undeliverable, compensated; the failed reservation is spendable again. Finality from another rail: `RAIL_MISMATCH`, recorded. The rail's retried report answers as before. |
| `09-refusal-flood` | B5 | Thirty-five invalid proofs against one context: every one refused `PROOF_INVALID`, the first 32 recorded and the rest `recorded false`; the chain holds exactly 32 refusals; the honest proof still pays and settles; the bounded chain verifies. |

## Writing one

`lib.sh` is the whole vocabulary. A scenario begins with `scenario NAME`,
which makes a working directory, a run suffix that every id carries, three
key pairs and the trust flags, and ends with `verify_evidence CTX` for each
chain that matters. In between:

```bash
step pay "$ctx" --reference "$(ref pay-ok)" --amount "128.00 INR"   # one process
expect_allowed "the proof pays" state=paid                           # exit 0, and these fields
expect_refused CART_BINDING_MISMATCH true "a swapped cart"           # exit 2, this code, recorded
expect_refused INVALID_STATE false "delivery before finality"        # exit 2, nothing to record
expect_undecided "unknown merchant key" "the adapter rejects it"     # exit 1, this on stderr
expect_chain "$ctx" "authorized denied:AMOUNT_MISMATCH paid"         # the chain, exactly
expect_contexts authorized 0 "nothing reserved"                      # the run's mandate, by state
```

A failed assertion prints the step, its exit code, its report and its
stderr, keeps the working directory and exits 1. `KEEP=1` keeps the
working directory of a passing scenario too. Lint with
`shellcheck -x -P SCRIPTDIR scenarios/*.sh`, as CI does.
