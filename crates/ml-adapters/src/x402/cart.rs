//! From an x402 cart document to a [`Cart`].

use super::assets::{Asset, AssetRegistry};
use super::fields::{address, malformed, money, uint, unsupported};
use super::types::{Address, ChainId, PaymentRequirements, ResourceInfo, X402Cart, account_id};
use ml_core::{
    AdapterError, Attestation, Cart, CartAdapter, MerchantId, Money, ScopeClaims, canonical_json,
};
use serde::Deserialize as _;

/// The longest icon URL, in UTF-16 code units, as the reference schema
/// bounds it.
const MAX_ICON_URL_LEN: usize = 2048;
/// Service names and tags: printable ASCII, this long at most.
const MAX_LABEL_LEN: usize = 32;
/// Tags on a resource, at most.
const MAX_TAGS: usize = 5;

/// An `exact` requirement on an EVM chain, checked and typed: what the
/// engine prices and what the rail will verify a payment against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactTerms {
    /// The chain.
    pub chain: ChainId,
    /// The token contract.
    pub asset: Address,
    /// The token, from the registry.
    pub token: Asset,
    /// The payee.
    pub pay_to: Address,
    /// The amount in atomic units.
    pub atomic_amount: u128,
    /// The amount as money, in the token's symbol.
    pub total: Money,
    /// How long the client has to pay, in seconds.
    pub max_timeout_seconds: u32,
}

/// Normalizes an [`X402Cart`] document into a [`Cart`].
#[derive(Clone, Debug)]
pub struct X402CartAdapter {
    registry: AssetRegistry,
}

/// A service name or a tag: 1 to 32 printable ASCII characters.
fn label(field: &str, value: &str) -> Result<(), AdapterError> {
    let printable = value.bytes().all(|b| (0x20..=0x7E).contains(&b));
    if value.is_empty() || value.len() > MAX_LABEL_LEN || !printable {
        return Err(malformed(&format!(
            "{field}: 1 to {MAX_LABEL_LEN} printable ASCII characters"
        )));
    }
    Ok(())
}

/// The resource, bounded exactly as the reference schema bounds it and no
/// further: none of it is a claim, so refusing what the reference accepts
/// would only refuse honest merchants.
fn resource(info: &ResourceInfo) -> Result<(), AdapterError> {
    if info.url.is_empty() {
        return Err(malformed("resource.url: required"));
    }
    if let Some(name) = &info.service_name {
        label("resource.serviceName", name)?;
    }
    if let Some(tags) = &info.tags {
        if tags.len() > MAX_TAGS {
            return Err(malformed("resource.tags: at most 5"));
        }
        for tag in tags {
            label("resource.tags", tag)?;
        }
    }
    if let Some(icon) = &info.icon_url {
        if icon.encode_utf16().count() > MAX_ICON_URL_LEN {
            return Err(malformed("resource.iconUrl: at most 2048 characters"));
        }
    }
    Ok(())
}

/// The `extra` object of an `exact` EVM requirement: the default transfer
/// method and payment flow — absent or `null` meaning the default, as the
/// reference resolves them — and the asset's own EIP-712 domain.
fn extra(extra: Option<&serde_json::Value>, token: &Asset) -> Result<(), AdapterError> {
    let required = || malformed("extra: name and version are required for eip3009");
    let fields = match extra {
        None => return Err(required()),
        Some(extra) => extra
            .as_object()
            .ok_or_else(|| malformed("extra: not an object"))?,
    };
    let reserved = |key: &str| fields.get(key).filter(|v| !v.is_null());
    if reserved("assetTransferMethod").is_some_and(|m| m.as_str() != Some("eip3009")) {
        return Err(unsupported("extra.assetTransferMethod: only eip3009"));
    }
    if reserved("paymentFlow").is_some_and(|f| f.as_str() != Some("authorization")) {
        return Err(unsupported("extra.paymentFlow: only authorization"));
    }
    let string = |key: &str| fields.get(key).and_then(serde_json::Value::as_str);
    match (string("name"), string("version")) {
        (Some(name), Some(version))
            if name == token.domain_name && version == token.domain_version =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(unsupported(
            "extra.name and extra.version: not the asset's EIP-712 domain",
        )),
        _ => Err(required()),
    }
}

impl X402CartAdapter {
    /// An adapter pricing the tokens in `registry`.
    #[must_use]
    pub fn new(registry: AssetRegistry) -> Self {
        Self { registry }
    }

    /// The tokens it prices.
    #[must_use]
    pub fn registry(&self) -> &AssetRegistry {
        &self.registry
    }

    /// Check a requirement and read it as [`ExactTerms`], refusing anything
    /// this adapter does not price exactly as the merchant meant it.
    pub fn terms(&self, accepted: &PaymentRequirements) -> Result<ExactTerms, AdapterError> {
        if accepted.scheme != "exact" {
            return Err(unsupported("scheme: only exact"));
        }
        let chain: ChainId = accepted.network.parse()?;
        let asset = address("asset", &accepted.asset)?;
        let token = self
            .registry
            .get(chain, asset)
            .ok_or_else(|| unsupported("asset: not a token this adapter knows on this chain"))?
            .clone();
        let pay_to = address("payTo", &accepted.pay_to)?;
        // Tokens sent to either are gone.
        if pay_to == Address::ZERO {
            return Err(malformed("payTo: the zero address"));
        }
        if pay_to == asset {
            return Err(malformed("payTo: the token contract itself"));
        }
        let atomic = uint("amount", &accepted.amount)?;
        let total = money(atomic, &token)?;
        if accepted.max_timeout_seconds == 0 {
            return Err(malformed("maxTimeoutSeconds: at least 1"));
        }
        extra(accepted.extra.as_ref(), &token)?;
        Ok(ExactTerms {
            chain,
            asset,
            token,
            pay_to,
            atomic_amount: atomic,
            total,
            max_timeout_seconds: accepted.max_timeout_seconds,
        })
    }
}

impl CartAdapter for X402CartAdapter {
    /// `raw` is the JSON of an [`X402Cart`]. The cart's bytes are its RFC
    /// 8785 canonical form, unknown fields inside `resource` and `accepted`
    /// included, so the hash binds what was presented, not only what was
    /// understood. RFC 8785 writes numbers through IEEE-754 doubles: every
    /// field the ledger reads is a string or a small integer, so no claim
    /// is touched, but two documents that differ only in the far digits of
    /// some other number are the same cart.
    fn normalize(&self, raw: &[u8]) -> Result<Cart, AdapterError> {
        let value: serde_json::Value =
            serde_json::from_slice(raw).map_err(|e| malformed(&format!("not JSON: {e}")))?;
        let document = X402Cart::deserialize(&value)
            .map_err(|e| malformed(&format!("not an x402 cart: {e}")))?;
        if document.x402_version != 2 {
            return Err(unsupported("x402Version: only 2"));
        }
        resource(&document.resource)?;
        let terms = self.terms(&document.accepted)?;
        let claims = ScopeClaims {
            merchant: MerchantId::new(account_id(terms.chain, terms.pay_to))?,
            total: terms.total,
            category: None,
            line_count: None,
        };
        Ok(Cart::new(
            canonical_json(&value)?,
            claims,
            Attestation::AgentReported,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x402::{BASE_SEPOLIA, ChainId};
    use ml_core::AttestationLevel;
    use serde_json::{Value, json};

    /// The example from the specification, Base Sepolia USDC.
    fn example() -> Value {
        json!({
            "x402Version": 2,
            "resource": {
                "url": "https://api.example.com/premium-data",
                "description": "Access to premium market data",
                "mimeType": "application/json"
            },
            "accepted": {
                "scheme": "exact",
                "network": "eip155:84532",
                "amount": "10000",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
                "maxTimeoutSeconds": 60,
                "extra": {"name": "USDC", "version": "2"}
            }
        })
    }

    /// The same purchase on Base mainnet.
    fn mainnet() -> Value {
        let value = with(example(), &["accepted", "network"], json!("eip155:8453"));
        let value = with(
            value,
            &["accepted", "asset"],
            json!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        );
        with(
            value,
            &["accepted", "extra"],
            json!({"name": "USD Coin", "version": "2"}),
        )
    }

    fn testnet() -> X402CartAdapter {
        X402CartAdapter::new(AssetRegistry::base_sepolia())
    }

    fn normalize(value: &Value) -> Result<Cart, AdapterError> {
        testnet().normalize(&serde_json::to_vec(value).unwrap())
    }

    fn with(mut value: Value, path: &[&str], field: Value) -> Value {
        let mut at = &mut value;
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        at[path[path.len() - 1]] = field;
        value
    }

    fn kind(err: &AdapterError) -> &'static str {
        match err {
            AdapterError::Malformed(_) => "Malformed",
            AdapterError::Unsupported(_) => "Unsupported",
            _ => "other",
        }
    }

    #[test]
    fn the_specification_example_is_a_cart() {
        let cart = normalize(&example()).unwrap();
        assert_eq!(
            cart.claims().merchant.as_str(),
            "eip155:84532:0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        assert_eq!(cart.claims().total.to_string(), "0.010000 USDC");
        assert_eq!(cart.claims().category, None);
        assert_eq!(cart.claims().line_count, None);
        assert_eq!(cart.attestation().level(), AttestationLevel::AgentReported);
    }

    #[test]
    fn the_same_payee_on_another_chain_is_another_merchant() {
        let base = X402CartAdapter::new(AssetRegistry::base());
        let on_mainnet = base
            .normalize(&serde_json::to_vec(&mainnet()).unwrap())
            .unwrap();
        let on_testnet = normalize(&example()).unwrap();
        assert_eq!(
            on_mainnet.claims().merchant.as_str(),
            "eip155:8453:0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        assert_ne!(on_mainnet.claims().merchant, on_testnet.claims().merchant);
        // A mainnet registry does not price the testnet token, and the
        // other way round.
        let err = base
            .normalize(&serde_json::to_vec(&example()).unwrap())
            .unwrap_err();
        assert_eq!(kind(&err), "Unsupported", "{err}");
        assert_eq!(kind(&normalize(&mainnet()).unwrap_err()), "Unsupported");
        // The testnet domain name on the mainnet token is refused.
        let wrong = with(
            mainnet(),
            &["accepted", "extra"],
            json!({"name": "USDC", "version": "2"}),
        );
        let err = base
            .normalize(&serde_json::to_vec(&wrong).unwrap())
            .unwrap_err();
        assert!(
            matches!(&err, AdapterError::Unsupported(e) if e.contains("EIP-712")),
            "{err}"
        );
    }

    #[test]
    fn the_hash_is_over_the_document_as_presented() {
        let cart = normalize(&example()).unwrap();
        // Key order does not matter; an unknown field inside does.
        let reordered: Value = serde_json::from_str(
            r#"{"accepted":{"extra":{"version":"2","name":"USDC"},"maxTimeoutSeconds":60,"payTo":"0x209693Bc6afc0C5328bA36FaF03C514EF312287C","asset":"0x036CbD53842c5426634e7929541eC2318f3dCF7e","amount":"10000","network":"eip155:84532","scheme":"exact"},"resource":{"mimeType":"application/json","description":"Access to premium market data","url":"https://api.example.com/premium-data"},"x402Version":2}"#,
        )
        .unwrap();
        assert_eq!(normalize(&reordered).unwrap().hash(), cart.hash());
        let extended = with(example(), &["accepted", "newField"], json!(1));
        let other = normalize(&extended).unwrap();
        assert_ne!(other.hash(), cart.hash());
        assert_eq!(other.claims(), cart.claims());
        // Echoed extensions are part of the purchase.
        let with_extensions = with(example(), &["extensions"], json!({"bazaar": {}}));
        assert_ne!(normalize(&with_extensions).unwrap().hash(), cart.hash());
    }

    #[test]
    fn a_cart_never_carries_the_signed_payment() {
        let signed = with(
            example(),
            &["payload"],
            json!({"signature": "0x00", "authorization": {}}),
        );
        let err = normalize(&signed).unwrap_err();
        assert!(
            matches!(&err, AdapterError::Malformed(e) if e.contains("payload")),
            "{err}"
        );
        let stray = with(example(), &["note"], json!("x"));
        assert_eq!(kind(&normalize(&stray).unwrap_err()), "Malformed");
    }

    #[test]
    fn amounts_are_exact_canonical_and_in_the_tokens_decimals() {
        for (atomic, money) in [
            ("1", "0.000001 USDC"),
            ("1500000", "1.500000 USDC"),
            ("0", "0.000000 USDC"),
        ] {
            let value = with(example(), &["accepted", "amount"], json!(atomic));
            assert_eq!(normalize(&value).unwrap().claims().total.to_string(), money);
        }
        for bad in [
            "", "-1", "1.5", "1e6", "10,000", " 1", "0x10", "010000", "00",
        ] {
            let value = with(example(), &["accepted", "amount"], json!(bad));
            assert_eq!(
                kind(&normalize(&value).unwrap_err()),
                "Malformed",
                "{bad:?}"
            );
        }
        let huge = with(example(), &["accepted", "amount"], json!("9".repeat(39)));
        assert_eq!(kind(&normalize(&huge).unwrap_err()), "Unsupported");
        let too_long = with(example(), &["accepted", "amount"], json!("1".repeat(79)));
        assert_eq!(kind(&normalize(&too_long).unwrap_err()), "Malformed");
    }

    #[test]
    fn only_what_the_adapter_prices_gets_through() {
        let cases: [(&[&str], Value, &str); 17] = [
            (&["x402Version"], json!(1), "Unsupported"),
            (&["accepted", "scheme"], json!("upto"), "Unsupported"),
            (
                &["accepted", "network"],
                json!("base-sepolia"),
                "Unsupported",
            ),
            (&["accepted", "network"], json!("eip155:1"), "Unsupported"),
            (
                &["accepted", "asset"],
                json!("0x0000000000000000000000000000000000000001"),
                "Unsupported",
            ),
            // Valid x402 this adapter does not support: a fiat asset, a
            // role constant as the payee.
            (&["accepted", "asset"], json!("USD"), "Unsupported"),
            (&["accepted", "payTo"], json!("merchant"), "Unsupported"),
            // Broken: not an address, or a typo in a checksummed one.
            (&["accepted", "payTo"], json!("0x1234"), "Malformed"),
            (
                &["accepted", "payTo"],
                json!("0x209693bC6afc0C5328bA36FaF03C514EF312287C"),
                "Malformed",
            ),
            // Lost funds.
            (
                &["accepted", "payTo"],
                json!("0x0000000000000000000000000000000000000000"),
                "Malformed",
            ),
            (
                &["accepted", "payTo"],
                json!("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
                "Malformed",
            ),
            (&["accepted", "maxTimeoutSeconds"], json!(0), "Malformed"),
            (&["accepted", "maxTimeoutSeconds"], json!(60.5), "Malformed"),
            (
                &["accepted", "extra"],
                json!({"name": "USDC", "version": "2", "assetTransferMethod": "permit2"}),
                "Unsupported",
            ),
            (
                &["accepted", "extra"],
                json!({"name": "USDC", "version": "2", "paymentFlow": "escrow"}),
                "Unsupported",
            ),
            (&["accepted", "extra"], json!({"name": "USDC"}), "Malformed"),
            (&["accepted", "extra"], json!(["USDC", "2"]), "Malformed"),
        ];
        for (path, field, expected) in cases {
            let err = normalize(&with(example(), path, field.clone())).unwrap_err();
            assert_eq!(kind(&err), expected, "{path:?} = {field}: {err}");
        }
        let mut no_extra = example();
        no_extra["accepted"]
            .as_object_mut()
            .unwrap()
            .remove("extra");
        assert_eq!(kind(&normalize(&no_extra).unwrap_err()), "Malformed");
        let mut no_resource = example();
        no_resource.as_object_mut().unwrap().remove("resource");
        assert_eq!(kind(&normalize(&no_resource).unwrap_err()), "Malformed");
        assert_eq!(kind(&testnet().normalize(b"[]").unwrap_err()), "Malformed");
        assert_eq!(
            kind(&testnet().normalize(b"{not json").unwrap_err()),
            "Malformed"
        );
    }

    #[test]
    fn the_defaults_may_be_spelt_out_or_null() {
        for extra in [
            json!({"name": "USDC", "version": "2", "assetTransferMethod": "eip3009", "paymentFlow": "authorization"}),
            json!({"name": "USDC", "version": "2", "assetTransferMethod": null, "paymentFlow": null}),
        ] {
            let value = with(example(), &["accepted", "extra"], extra.clone());
            assert!(normalize(&value).is_ok(), "{extra}");
        }
    }

    #[test]
    fn the_resource_is_bounded_as_the_reference_bounds_it() {
        // What the reference schema accepts is accepted: empty and
        // multi-line text, long URLs, explicit nulls, any icon scheme.
        for (path, field) in [
            (&["resource", "description"][..], json!("")),
            (&["resource", "description"][..], json!("two\nlines")),
            (&["resource", "description"][..], Value::Null),
            (&["resource", "mimeType"][..], json!("")),
            (&["resource", "mimeType"][..], Value::Null),
            (
                &["resource", "url"][..],
                json!(format!("https://a.example/?q={}", "x".repeat(4000))),
            ),
            (
                &["resource", "serviceName"][..],
                json!("Example Market Data"),
            ),
            (&["resource", "serviceName"][..], Value::Null),
            (&["resource", "tags"][..], json!(["market-data", "finance"])),
            (&["resource", "tags"][..], Value::Null),
            (&["resource", "iconUrl"][..], json!("")),
            (&["resource", "iconUrl"][..], json!("ipfs://icon")),
            (&["resource", "iconUrl"][..], Value::Null),
            (&["extensions"][..], Value::Null),
        ] {
            let value = with(example(), path, field.clone());
            assert!(normalize(&value).is_ok(), "{path:?} = {field}");
        }
        // What it refuses is refused.
        for (path, field) in [
            (&["resource", "url"][..], json!("")),
            (&["resource", "serviceName"][..], json!("x".repeat(33))),
            (&["resource", "serviceName"][..], json!("caf\u{e9}")),
            (&["resource", "serviceName"][..], json!("")),
            (
                &["resource", "tags"][..],
                json!(["a", "b", "c", "d", "e", "f"]),
            ),
            (&["resource", "tags"][..], json!([""])),
            (&["resource", "iconUrl"][..], json!("x".repeat(2049))),
        ] {
            let err = normalize(&with(example(), path, field.clone())).unwrap_err();
            assert_eq!(kind(&err), "Malformed", "{path:?} = {field}: {err}");
        }
    }

    #[test]
    fn terms_carry_what_the_rail_will_need() {
        let document: X402Cart = serde_json::from_value(example()).unwrap();
        let terms = testnet().terms(&document.accepted).unwrap();
        assert_eq!(terms.chain, BASE_SEPOLIA);
        assert_ne!(terms.chain, ChainId::new(8453));
        assert_eq!(terms.atomic_amount, 10_000);
        assert_eq!(terms.token.decimals, 6);
        assert_eq!(terms.token.domain_name, "USDC");
        assert_eq!(
            terms.pay_to.to_string(),
            "0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        assert_eq!(terms.max_timeout_seconds, 60);
    }
}
