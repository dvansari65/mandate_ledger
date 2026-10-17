//! The engine behind a trait object. The handlers do not care which store
//! is underneath: the service runs over PostgreSQL, the tests over
//! `MemoryStore`, through the same code.

use ml_adapters::MockProof;
use ml_adapters::sandbox::Finality;
use ml_core::{
    Authorized, Cart, Clock, Compensated, ContextId, Delivered, DeliveryReceipt, Event,
    EvidenceBundle, Ledger, LedgerError, Mandate, MandateId, Paid, Rail, Reached, Record,
    RecordFilter, Settled, Settlement, SettlementFailed, SignerPolicy, Store, StoreError,
};

/// What the service asks of the engine: the five calls, the two operator
/// actions, and the read surface.
pub trait Backend: Send + Sync + 'static {
    /// [`Ledger::authorize`].
    fn authorize(
        &self,
        mandate: &Mandate,
        cart: &Cart,
        request_key: &str,
    ) -> Result<Authorized, LedgerError>;
    /// [`Ledger::record_payment`], with the sandbox rail's proof.
    fn record_payment(&self, auth: &Authorized, proof: &MockProof) -> Result<Paid, LedgerError>;
    /// [`Ledger::record_settlement`], with the sandbox rail's finality.
    fn record_settlement(
        &self,
        paid: &Paid,
        finality: &Finality,
    ) -> Result<Settlement, LedgerError>;
    /// [`Ledger::compensate`].
    fn compensate(
        &self,
        failed: &SettlementFailed,
        reference: Option<&str>,
    ) -> Result<Compensated, LedgerError>;
    /// [`Ledger::record_delivery`].
    fn record_delivery(
        &self,
        settled: &Settled,
        receipt: DeliveryReceipt,
    ) -> Result<Delivered, LedgerError>;
    /// [`Ledger::expire`].
    fn expire(&self, auth: &Authorized) -> Result<(), LedgerError>;
    /// [`Ledger::revoke`].
    fn revoke(&self, mandate: &MandateId) -> Result<(), LedgerError>;
    /// [`Ledger::reached`].
    fn reached(&self, ctx: &ContextId) -> Result<Option<Reached>, LedgerError>;
    /// [`Ledger::evidence`].
    fn evidence(&self, ctx: &ContextId) -> Result<Option<EvidenceBundle>, LedgerError>;
    /// [`Store::record`].
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError>;
    /// [`Store::scan`].
    fn scan(
        &self,
        filter: &RecordFilter,
        after: Option<&ContextId>,
        limit: usize,
    ) -> Result<Vec<Record>, StoreError>;
    /// [`Store::events_after`].
    fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>, StoreError>;
    /// [`Store::last_seq`].
    fn last_seq(&self) -> Result<u64, StoreError>;
}

impl<S, R, P, C> Backend for Ledger<S, R, P, C>
where
    S: Store + 'static,
    R: Rail<Proof = MockProof, Finality = Finality> + 'static,
    P: SignerPolicy + 'static,
    C: Clock + 'static,
{
    fn authorize(
        &self,
        mandate: &Mandate,
        cart: &Cart,
        request_key: &str,
    ) -> Result<Authorized, LedgerError> {
        Ledger::authorize(self, mandate, cart, request_key)
    }
    fn record_payment(&self, auth: &Authorized, proof: &MockProof) -> Result<Paid, LedgerError> {
        Ledger::record_payment(self, auth, proof)
    }
    fn record_settlement(
        &self,
        paid: &Paid,
        finality: &Finality,
    ) -> Result<Settlement, LedgerError> {
        Ledger::record_settlement(self, paid, finality)
    }
    fn compensate(
        &self,
        failed: &SettlementFailed,
        reference: Option<&str>,
    ) -> Result<Compensated, LedgerError> {
        Ledger::compensate(self, failed, reference)
    }
    fn record_delivery(
        &self,
        settled: &Settled,
        receipt: DeliveryReceipt,
    ) -> Result<Delivered, LedgerError> {
        Ledger::record_delivery(self, settled, receipt)
    }
    fn expire(&self, auth: &Authorized) -> Result<(), LedgerError> {
        Ledger::expire(self, auth)
    }
    fn revoke(&self, mandate: &MandateId) -> Result<(), LedgerError> {
        Ledger::revoke(self, mandate)
    }
    fn reached(&self, ctx: &ContextId) -> Result<Option<Reached>, LedgerError> {
        Ledger::reached(self, ctx)
    }
    fn evidence(&self, ctx: &ContextId) -> Result<Option<EvidenceBundle>, LedgerError> {
        Ledger::evidence(self, ctx)
    }
    fn record(&self, ctx: &ContextId) -> Result<Option<Record>, StoreError> {
        self.store().record(ctx)
    }
    fn scan(
        &self,
        filter: &RecordFilter,
        after: Option<&ContextId>,
        limit: usize,
    ) -> Result<Vec<Record>, StoreError> {
        self.store().scan(filter, after, limit)
    }
    fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>, StoreError> {
        self.store().events_after(after, limit)
    }
    fn last_seq(&self) -> Result<u64, StoreError> {
        self.store().last_seq()
    }
}
