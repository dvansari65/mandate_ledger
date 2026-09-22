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
  | `2` | The ledger evaluated the step and **refused** it. A decision, not a failure; the report names the context, the stage, the code and the reason. |
  | `1` | The ledger could not decide: a file, the store or the rail failed. Fix the cause and retry. |
  | `64` | The command line was wrong. Distinct from a refusal on purpose. |

## Keys

```bash
ml keys new --out user.key
```

Writes an Ed25519 key pair as JSON — `algorithm`, `public`, `secret`, hex —
readable by its owner alone on Unix, and never overwrites an existing file. The
secret is in the clear: this is a sandbox key. Make one for the principal
(the wallet) and one for each merchant.

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
holds — which is exactly what a service restarting would see.

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
  --trust user:alice=user.key --merchant-key bb-2026=merchant.key
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
context  ctx_3ea14003a08a0378c251260d414ad731
stage    authorize
refused  SCOPE_PER_TXN_EXCEEDED
detail   total 3000.00 INR exceeds per-transaction cap 2000.00 INR
```

A cart the adapter rejects — a bad or unknown merchant signature — never
reaches the ledger; that is exit `1` and an error, not a recorded refusal.

## Log and contexts

```bash
ml log                        # the most recent page of the global log
ml log --after 120 --limit 20 # page forward; the report's `next` is the cursor
ml log --context ctx_…        # one context's chain, oldest first
ml contexts --mandate mnd-1 --state authorized
```

`ml log` is every event in every context, refusals included, in the order
the store committed them. `ml contexts` is where each context stands now: a
context that has only ever been refused has no record — its refusals are in
the log.
