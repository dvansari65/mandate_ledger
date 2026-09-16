# Integration guide

mandate-ledger is a library that runs **inside** whichever service decides
whether a payment step may proceed. It never calls your PSP or a chain; you
do. It gates and records.

## 1. Choose your four dependencies

```rust
let ledger = Ledger::new(store, rail, signers, clock);
```

| Slot | Trait | Start with | Production |
|---|---|---|---|
| `store` | `Store` | `MemoryStore` | `PostgresStore` from `ml-store-postgres` — durable, and `append` is one transaction |
| `rail` | `Rail` | `MockRail` | one per money-mover: `RazorpayRail`, `X402Rail`… |
| `signers` | `SignerPolicy` | `TrustedSigners` | your user-key directory |
| `clock` | `Clock` | `SystemClock` | `SystemClock` |

Several ledgers can share one store (wrap it in `Arc`) — e.g. one per rail.
Evidence from rail A is never accepted for a payment made on rail B.

## 2. Get a `Cart`

Never build `ScopeClaims` by hand from agent-supplied JSON. Use a
`CartAdapter` that (a) canonicalizes, (b) verifies the merchant signature
if there is one, and (c) only fills a claim the protocol actually carries.

```rust
let adapter = NativeCartAdapter::new().with_merchant_key("bb-2026", merchant_pubkey);
let cart = adapter.normalize(&body_bytes)?;
```

If your protocol can't say what category a cart is, leave `category: None`.
A mandate that constrains category will then deny `UNVERIFIABLE_SCOPE`.
That is correct. Do not guess.

## 3. Authorize (request handler)

```rust
let auth = match ledger.authorize(&mandate, &cart, &request_key) {
    Ok(a) => a,
    Err(LedgerError::Denied(d)) => return respond_403(d.reason.code(), &d.detail),
    Err(e) => return respond_503(e),   // store/rail outage — client may retry
};
// Persist auth.ctx() alongside your order. Put it in the PSP order's
// receipt/metadata so the webhook can carry it back.
```

`request_key` is *your* idempotency key for this purchase attempt. Same
mandate + same cart + same key → same context, nothing reserved twice.

## 4. Record payment (webhook handler)

```rust
let Some(Resumed::Authorized(auth)) = ledger.resume(&ctx)? else { return ignore() };
let paid = ledger.record_payment(&auth, &proof)?;
```

Your `Rail::verify_proof` must return a `VerifiedProof` with at least one of
`bound_ctx` / `bound_cart` populated. For a PSP, that means reading your
context id back out of the payment's metadata. A proof that binds to
nothing is denied `UNBOUND_PROOF`.

## 5. Settle, then deliver

```rust
match ledger.record_settlement(&paid, &finality)? {
    Settlement::Settled(s) => {
        fulfil(&order);                                   // only now
        ledger.record_delivery(&s, receipt)?;
    }
    Settlement::Pending { reason } => schedule_retry(reason),
    Settlement::Failed(f) => {
        undo_side_effects(&order);
        ledger.compensate(&f, Some(&refund_id))?;
    }
}
```

What "final" means is the rail's decision: `captured` for a card PSP,
N confirmations for a chain. Encode it in `Rail::check_finality`.

## 6. Release what won't be paid

If a customer abandons after `authorize`, call `ledger.expire(&auth)` so the
reservation returns to the mandate's budget. A background sweep over
`Authorized` contexts older than your checkout window is the usual shape.

## 7. Disputes

```rust
let bundle = ledger.evidence(&ctx)?.unwrap();
let signed = bundle.sign(&host_key)?;          // optional
serde_json::to_string(&signed)?
```

The bundle carries the signed mandate, the cart as authorized, every
transition, and every refusal. Anyone can call `verify()` on it without
your database.

## Error handling in one rule

```rust
match err {
    LedgerError::Denied(d)        => /* final answer; show d.reason, do not retry */,
    LedgerError::Store(_)
    | LedgerError::RailUnavailable(_) => /* transient; retry with backoff */,
    LedgerError::Canonicalize(_)  => /* bug in an adapter; alert */,
}
```

## Writing a `Rail`

```rust
impl Rail for RazorpayRail {
    type Proof = RazorpayWebhook;      // the parsed, signature-verified webhook
    type Finality = RazorpayWebhook;   // same event stream

    fn id(&self) -> &str { "razorpay" }

    fn verify_proof(&self, p: &Self::Proof, _e: &PaymentExpectation<'_>) -> Result<VerifiedProof, RailError> {
        // Do: check webhook HMAC, parse amount, read ctx id from notes/receipt.
        // Don't: compare amount to `_e.amount` — the engine does that and records why.
        Ok(VerifiedProof {
            reference: p.payment_id.clone(),
            nonce: p.payment_id.clone(),       // PSP ids are unique per payment
            amount: p.amount()?,
            bound_ctx: p.notes.get("ml_ctx").map(ContextId::new).transpose()?,
            bound_cart: None,
        })
    }

    fn check_finality(&self, f: &Self::Finality, e: &SettlementExpectation<'_>) -> Result<FinalityStatus, RailError> {
        if f.payment_id != e.payment_reference {
            return Err(RailError::InvalidProof("event is for a different payment".into()));
        }
        Ok(match f.event.as_str() {
            "payment.captured" => FinalityStatus::Final { reference: f.event_id.clone() },
            "payment.failed"   => FinalityStatus::Failed { reason: f.error_description.clone() },
            _                  => FinalityStatus::Pending { reason: f.event.clone() },
        })
    }
}
```
