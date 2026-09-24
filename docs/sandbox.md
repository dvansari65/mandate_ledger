# Sandbox

The `ml` command drives the engine from a terminal. Every command is one
process, so a lifecycle that spans commands is a lifecycle that survived a
restart. This page is the operator's guide; it grows with the command.

```bash
cargo install --path crates/ml-sandbox     # or: cargo run -q -p ml-sandbox -- <args>
```

## Conventions

Every command follows these, and scripts rely on them.

- **State is files and the database, nothing else.** Keys, mandates and carts
  are JSON files you can read and edit. Nothing is remembered between two
  commands except what the store holds.
- **`--json`** prints one JSON object on stdout in place of the human-readable
  `label  value` lines, with the same keys. Errors always go to stderr,
  prefixed `error:`.
- **Exit codes mean something.**

  | Code | Meaning |
  |---|---|
  | `0` | The step was allowed, or the command had nothing to decide. |
  | `2` | The ledger evaluated the step and **refused** it, or an evidence bundle did not verify. A decision, not a failure; the report names the code and the reason. |
  | `1` | The ledger could not decide: a file, the store or the rail failed. Fix the cause and retry. |
  | `64` | The command line was wrong. Distinct from a refusal on purpose. |

## Keys

```bash
ml keys new --out user.key
```

Writes an Ed25519 key pair as JSON — `algorithm`, `public`, `secret`, hex —
to `user.key`, readable by its owner alone on Unix and never overwritten, and
the public half alone to `user.key.pub`. Anything that only needs to trust a
key takes the `.pub` file; the private file never has to leave the machine
that signs with it. The secret is in the clear: this is a sandbox key. Make
one pair for the principal (the wallet) and one for each merchant.

## Mandates

```bash
ml mandate sign body.json --key user.key --out mandate.json
```

`body.json` is the mandate body exactly as the engine signs it. Nothing is
filled in for you: a mandate is a statement of authority, and every field of
it should be deliberate.

```json
{
  "id": "mnd-1",
  "principal": "user:alice",
  "agent": "agent:shopper",
  "scope": {
    "merchants": ["bigbasket.com", "*.zepto.com"],
    "categories": ["grocery"],
    "currency": "INR",
    "max_per_txn": { "amount": "2000.00", "currency": "INR" },
    "max_total": { "amount": "8000.00", "currency": "INR" },
    "valid_from": 1800000000,
    "valid_until": 1802592000,
    "velocity": { "max_count": 5, "window_secs": 86400 },
    "min_attestation": "merchant_signed"
  },
  "issued_at": 1800000000
}
```

`categories`, `max_per_txn`, `max_total`, `velocity` and `parent` may be
omitted; `merchants` accepts `"*"`, an exact id, or `"*.suffix"`; times are
Unix seconds; `min_attestation` is `agent_reported` or `merchant_signed`.
The output is the signed mandate the engine accepts — the body plus the
signer's public key and the signature, as hex. An invalid scope (a window
that ends before it starts, a cap in another currency) is an error, not a
mandate.

## The database

Every command that decides or reads needs PostgreSQL:

```bash
export ML_DATABASE_URL=postgres://localhost/mandate_ledger   # or --database-url on each command
```

That is the only setting. Each command opens one connection, brings the
schema up to date (a version check; nothing happens when it is current) and
exits. Nothing is remembered between two commands except what the database
holds — which is exactly what a service restarting would see. A database that
cannot be reached is an error within five seconds, with the cause.

## Carts

```bash
ml cart sign cart.json --key merchant.key --key-id bb-2026 --out signed-cart.json
```

`cart.json` is the native cart format — `merchant`, `total`, optional
`category`, opaque `items`:

```json
{
  "merchant": "bigbasket.com",
  "total": { "amount": "128.00", "currency": "INR" },
  "category": "grocery",
  "items": [ { "sku": "milk-1l", "qty": 2 } ]
}
```

`items` is hashed, never read. A mandate whose `min_attestation` is
`merchant_signed` accepts only carts the merchant signed; `ml cart sign`
adds that signature under a key id, and `--merchant-key ID=KEYFILE` on
`authorize` tells the engine which key that id means.

## Authorize

```bash
ml authorize --mandate mandate.json --cart signed-cart.json --request-key order-1 \
  --trust user:alice=user.key.pub --merchant-key bb-2026=merchant.key.pub
```

Checks the cart against the mandate and reserves its total. Prints the
context id — the handle every later step takes — the merchant and the
amount. `--request-key` is the idempotency key for this purchase attempt:
the same mandate, cart and key from any process land on the same context and
reserve nothing twice.

`--trust PRINCIPAL=KEYFILE` is the signer policy: which key may sign
mandates for which principal. Without it every mandate is refused
`MANDATE_SIGNER_UNTRUSTED`, because a mandate cannot vouch for its own key.
That is how production works, and the sandbox does not relax it.

A refusal is exit `2` and a report:

```
context   ctx_3ea14003a08a0378c251260d414ad731
stage     authorize
refused   SCOPE_PER_TXN_EXCEEDED
detail    total 3000.00 INR exceeds per-transaction cap 2000.00 INR
recorded  true
```

A cart the adapter rejects — a bad or unknown merchant signature — never
reaches the ledger; that is exit `1` and an error, not a recorded refusal.

## Pay, settle, deliver

Every later step takes the context id from `ml authorize`. Each command is
a new process: it reads the context back from the database, presents the
token the step needs, and reports the engine's decision.

```bash
ml pay ctx_… --reference pay-1 --amount "128.00 INR"
ml settle ctx_… --confirmations 1            # or: --failed reverted
ml deliver ctx_… --receipt BB-88121 --signed-by bb-2026
```

`pay` records the mock rail's proof. Its flags are what a real rail would
decide for itself, and they are how the attack scripts drive the engine:
`--nonce` (defaults to the reference; present one for a second context to
replay a payment), `--bound-cart HASH` (the hash of another cart is a swap —
`ml cart sign` and `ml authorize` both print the hash), `--unbound`,
`--invalid`. Every one of them is refused by the engine, not by the command.

`settle` asks the rail for finality: `--confirmations N` is final at 1 and
`pending` below that — nothing is recorded, ask again later — and
`--failed REASON` releases the reservation. `--reference` presents evidence
about another payment, which is refused. After a failure:

```bash
ml compensate ctx_… --reference refund-1
```

`deliver` needs a settled context: the engine's `record_delivery` takes a
`Settled` token and nothing else. On any other context the command has
nothing to present, and says so — see below.

## Expire and revoke

```bash
ml expire ctx_…      # release an authorization that will not be paid
ml revoke mnd-1      # every later authorization under the mandate is refused
```

## Replays, and refusals the ledger never sees

Every step replays. `pay` with the same proof, `settle` after delivery,
`deliver` twice: the engine answers as it did the first time and adds nothing
to the chain. A *different* payment against a paid context, or expiring a
context that has moved on, is the engine's refusal, recorded, and the report
says `recorded  true`.

Some refusals never reach the ledger: a step on a context that does not
exist, or one the context never earned the token for — a delivery before
settlement, a settlement of a context that was never paid. The type system
says no before the engine can be asked, so there is no event to record. The
report carries the engine's code for it (`INVALID_STATE`,
`CONTEXT_NOT_FOUND`) and `recorded  false`.

## Log and contexts

```bash
ml log                        # the most recent 50 events, every context
ml log --after 120 --limit 20 # page forward; the report's `next` is the cursor
ml log --limit 0              # nothing but `next`: where the log ends now
ml log --context ctx_…        # one context's chain, oldest first
ml contexts --mandate mnd-1 --state authorized
```

`ml log` is every event in every context, refusals included, in the order
the store committed them. Each row carries `seq`, `at`, `context`, `event`,
`code` — the refusal code for a `denied` event, empty otherwise — and
`detail`. A script that wants everything from a point in time takes `next`
from `ml log --limit 0` and passes it to `--after` later. `ml contexts` is
where each context stands now: a context that has only ever been refused has
no record — its refusals are in the log.

## Evidence

```bash
ml evidence ctx_… --out bundle.json                  # the chain, as a file
ml evidence ctx_… --out bundle.json --sign host.key  # signed by the exporting host
ml verify bundle.json                                # no database, no network
ml verify bundle.json --signer host.key.pub          # and it must be this host's
```

`ml evidence` exports a context's whole chain — every event, hashes and all
— as a bundle. Signed with the host's key, the file also names who exported
it. `ml verify` recomputes every hash and follows every link, and checks the
signature if there is one, from nothing but the file: it takes no database
flags, and it runs on a machine that has never seen the ledger. That is the
claim "anyone can verify it without your database", made runnable.

A bundle that does not verify is exit `2`, with the code and the event it
failed at — `event` is the sequence number, as it appears in the file:
`HASH_MISMATCH` (an event no longer matches its hash), `BROKEN_CHAIN` (an
event was removed or reordered), `SEQUENCE_NOT_INCREASING`, `WRONG_CONTEXT`,
`EMPTY`, `UNSUPPORTED_VERSION` (a format this build does not know is refused,
not checked with the wrong rules), or `SIGNATURE_INVALID` — which also covers
a bundle signed by someone other than `--signer`, and a bundle with no
signature when `--signer` is given. A file that is not a bundle at all is an
error, exit `1`.

The chain check catches an edit to any event. It cannot catch an edit that
recomputes every hash afterwards — hashes have no secret in them, so anyone
can rebuild a consistent chain. That is what the signature is for: a
re-hashed bundle is intact by the chain's rules and fails the host's
signature. Sign what you hand to a third party.
