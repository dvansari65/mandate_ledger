# Roadmap

What it takes to get from here to a production-grade 0.1, in dependency
order. The idea every item serves: an enforcement layer an integrator puts
between an agent and a payment rail, which refuses any step where the
mandate, the cart, the proof and the finality disagree, and records why.

## 0.1 — "put this in front of an x402 payment in a sandbox"

### 1. Finish the harness
- [x] Read surface: `events_after`, `scan`, `last_seq`; the log in commit order
- [x] `ml` — keys, mandates, carts, `authorize`, `log`, `contexts`
- [x] `ml pay` / `settle` / `compensate` / `deliver` / `expire` / `revoke`
- [x] `ml evidence` / `ml verify` — verification with no database
- [x] Controllable rail (`…-ok`, `…-fail`, `…-reorg`), `--rail`, and a test clock
- [x] Eight scenario scripts, one per threat-model row, run by a CI job
      against the PostgreSQL service

### 2. Close the engine gaps found in review
- [x] Payee binding: `VerifiedProof.bound_merchant`, `MERCHANT_BINDING_MISMATCH`
- [x] B4: `authorize` replay compares the stored mandate — `CONTEXT_MISMATCH`
- [x] B6: cap `Denied` events per context, live ones included — a retry
      under a forged mandate lands on the chain it names (the audit path is
      a storage DoS)
- [x] Keyset paging for `scan`; `MemoryStore` pruning; the test suite's
      connection budget

### 3. Hygiene
- [x] `SECURITY.md`, `CHANGELOG.md`, `#![deny(missing_docs)]` in the library
      crates, a stability policy for the refusal codes and wire formats,
      crates.io metadata, a release process
- [ ] Branch protection on `main`: import `.github/rulesets/main.json` from
      Settings → Rules → Rulesets, and enable private vulnerability
      reporting, which `SECURITY.md` relies on
- [ ] Before 0.1, the other half of the stability policy: `#[non_exhaustive]`
      on the remaining public enums, and constructors for the structs an
      integrator builds (`VerifiedProof`, `DeliveryReceipt`), so that adding
      a field is not a breaking change

### 4. The service and the SDK — the adoption path
- [x] `ml-serve`: the five calls over HTTP; status codes mirror the exit
      codes (200 allowed, 409 refused with the code, 503 could not decide);
      request keys; health and readiness; structured logs; graceful
      shutdown; configuration from the environment; OpenAPI
- [ ] Metrics; per-key rate limits — the admission control that bounds
      refusal flooding of new contexts (B5); the finality webhook, once a
      rail has something to call it with
- [ ] Docker image and `compose.yaml` with PostgreSQL
- [ ] A TypeScript SDK generated from the OpenAPI spec, with the refusal
      codes as a typed union, and a Node example
- Decided: API keys, with mTLS as a later option; the synchronous store
  behind blocking threads with as many permits as the pool has
  connections; one ledger per deployment

### 5. The first real adapter: x402
- [x] 402 terms → cart (`payTo` as the merchant, so §2's payee binding first):
      `X402CartAdapter` and the asset registry, pinned to x402 v2
- [ ] Signed payment payload → proof (its nonce, the terms hash, `payTo`)
- [ ] Chain receipt → finality; one rail id per network
- [ ] The eight scenarios through x402 payloads, plus spec test vectors
- Decisions: facilitator or direct chain verification; first network

## After 0.1

### 6. Money model
Partial captures, refunds as transitions under scope and velocity, currency
exponents, clock-skew tolerance, automatic expiry of lapsed authorizations.
Each adds a threat-model row and a scenario.

### 7. Trust
Delegation-chain verification (`parent` is written, never checked), key ids
and rotation with validity windows, a signed revocation list verifiable
offline, evidence redaction for third parties.

### 8. Operability
TLS to PostgreSQL (the store's pool is plaintext), retention for
`ml_authorizations`, index review under load, backup and restore guidance,
pool sizing, published benchmarks, a multi-rail registry.

### 9. Assurance, before 1.0
Fuzzing the adapters, property tests that the PostgreSQL store is
indistinguishable from the in-memory one, model checking of the lifecycle,
an external audit. Nothing carries real money before the audit.

### 10. The remaining adapters
AP2, ACP, MPP — each proven by the same eight scenarios.
