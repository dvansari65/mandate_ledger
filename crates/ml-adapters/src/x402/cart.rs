//! From an x402 cart document to a [`Cart`].

use super::assets::{Asset, AssetRegistry};
use super::types::{Address, ChainId, PaymentRequirements, ResourceInfo, X402Cart};
use ml_core::{
    AdapterError, Attestation, Cart, CartAdapter, MerchantId, Money, ScopeClaims, canonical_json,
};
use rust_decimal::Decimal;

/// The most digits a `uint256` has.
const MAX_AMOUNT_DIGITS: usize = 78;
/// The longest URL accepted, as the reference implementation bounds its own.
const MAX_URL_LEN: usize = 2048;
/// The longest free text accepted in a resource description or media type.
const MAX_TEXT_LEN: usize = 1024;
/// Service names and tags: printable ASCII, this long at most, as the
/// specification says.
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

fn malformed(what: &str) -> AdapterError {
    AdapterError::Malformed(what.to_owned())
}

fn unsupported(what: &str) -> AdapterError {
    AdapterError::Unsupported(what.to_owned())
}

fn printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7E).contains(&b))
}

/// A string field: present, non-empty, bounded, and free of control
/// characters.
fn text(field: &str, value: &str, max: usize) -> Result<(), AdapterError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(malformed(&format!(
            "{field}: 1 to {max} bytes, no control characters"
        )));
    }
    Ok(())
}

fn label(field: &str, value: &str) -> Result<(), AdapterError> {
    if value.is_empty() || value.len() > MAX_LABEL_LEN || !printable_ascii(value) {
        return Err(malformed(&format!(
            "{field}: 1 to {MAX_LABEL_LEN} printable ASCII characters"
        )));
    }
    Ok(())
}

fn resource(info: &ResourceInfo) -> Result<(), AdapterError> {
    text("resource.url", &info.url, MAX_URL_LEN)?;
    if info.url.contains(char::is_whitespace) {
        return Err(malformed("resource.url: not a URL"));
    }
    if let Some(description) = &info.description {
        text("resource.description", description, MAX_TEXT_LEN)?;
    }
    if let Some(mime_type) = &info.mime_type {
        text("resource.mimeType", mime_type, MAX_TEXT_LEN)?;
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
        text("resource.iconUrl", icon, MAX_URL_LEN)?;
        if !(icon.starts_with("https://") || icon.starts_with("http://")) {
            return Err(malformed("resource.iconUrl: not an http or https URL"));
        }
    }
    Ok(())
}

/// The amount in atomic units: a whole decimal number of at most 78
/// digits, the range of a `uint256`, within what this adapter prices.
fn atomic_amount(amount: &str) -> Result<u128, AdapterError> {
    let digits = !amount.is_empty()
        && amount.len() <= MAX_AMOUNT_DIGITS
        && amount.bytes().all(|b| b.is_ascii_digit());
    if !digits {
        return Err(malformed("amount: a whole number of atomic units"));
    }
    amount
        .parse()
        .map_err(|_| unsupported("amount: beyond the range this adapter prices"))
}

/// Atomic units into money, exactly: the token's decimals become the scale.
fn money(atomic: u128, token: &Asset) -> Result<Money, AdapterError> {
    let units = i128::try_from(atomic)
        .ok()
        .and_then(|units| Decimal::try_from_i128_with_scale(units, u32::from(token.decimals)).ok())
        .ok_or_else(|| unsupported("amount: beyond the range this adapter prices"))?;
    Ok(Money::new(units, token.symbol.clone()))
}

/// The `extra` object of an `exact` EVM requirement: the default transfer
/// method and payment flow, and the asset's own EIP-712 domain.
fn extra(extra: Option<&serde_json::Value>, token: &Asset) -> Result<(), AdapterError> {
    let Some(extra) = extra else {
        return Err(malformed(
            "extra: name and version are required for eip3009",
        ));
    };
    let Some(fields) = extra.as_object() else {
        return Err(malformed("extra: not an object"));
    };
    let string = |key: &str| fields.get(key).and_then(serde_json::Value::as_str);
    match fields.get("assetTransferMethod") {
        None => {}
        Some(method) if method.as_str() == Some("eip3009") => {}
        Some(_) => return Err(unsupported("extra.assetTransferMethod: only eip3009")),
    }
    match fields.get("paymentFlow") {
        None => {}
        Some(flow) if flow.as_str() == Some("authorization") => {}
        Some(_) => return Err(unsupported("extra.paymentFlow: only authorization")),
    }
    match (string("name"), string("version")) {
        (Some(name), Some(version))
            if name == token.domain_name && version == token.domain_version =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(unsupported(
            "extra.name and extra.version: not the asset's EIP-712 domain",
        )),
        _ => Err(malformed(
            "extra: name and version are required for eip3009",
        )),
    }
}

impl X402CartAdapter {
    /// An adapter pricing the tokens in `registry`.
    #[must_use]
    pub fn new(registry: AssetRegistry) -> Self {
        Self { registry }
    }

    /// The registry.
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
        let asset: Address = accepted
            .asset
            .parse()
            .map_err(|_| malformed("asset: not an EVM address"))?;
        let token = self
            .registry
            .get(chain, asset)
            .ok_or_else(|| unsupported("asset: not a token this adapter knows on this chain"))?
            .clone();
        let pay_to: Address = accepted
            .pay_to
            .parse()
            .map_err(|_| malformed("payTo: not an EVM address"))?;
        let atomic = atomic_amount(&accepted.amount)?;
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

    /// Normalize an already-parsed document whose canonical bytes are `raw`.
    fn normalize_document(&self, document: &X402Cart, raw: Vec<u8>) -> Result<Cart, AdapterError> {
        if document.x402_version != 2 {
            return Err(unsupported("x402Version: only 2"));
        }
        resource(&document.resource)?;
        let terms = self.terms(&document.accepted)?;
        let claims = ScopeClaims {
            merchant: MerchantId::new(terms.pay_to.to_string())?,
            total: terms.total,
            category: None,
            line_count: None,
        };
        Ok(Cart::new(raw, claims, Attestation::AgentReported))
    }
}

impl CartAdapter for X402CartAdapter {
    /// `raw` is the JSON of an [`X402Cart`]. The cart's bytes are its
    /// canonical form, unknown fields and all, so the hash binds what was
    /// presented, not only what was understood.
    fn normalize(&self, raw: &[u8]) -> Result<Cart, AdapterError> {
        let value: serde_json::Value =
            serde_json::from_slice(raw).map_err(|e| malformed(&format!("not JSON: {e}")))?;
        let document: X402Cart = serde_json::from_value(value.clone())
            .map_err(|e| malformed(&format!("not an x402 cart: {e}")))?;
        self.normalize_document(&document, canonical_json(&value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn adapter() -> X402CartAdapter {
        X402CartAdapter::new(AssetRegistry::base())
    }

    fn normalize(value: &Value) -> Result<Cart, AdapterError> {
        adapter().normalize(&serde_json::to_vec(value).unwrap())
    }

    fn with(mut value: Value, path: &[&str], field: Value) -> Value {
        let mut at = &mut value;
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        at[path[path.len() - 1]] = field;
        value
    }

    #[test]
    fn the_specification_example_is_a_cart() {
        let cart = normalize(&example()).unwrap();
        assert_eq!(
            cart.claims().merchant.as_str(),
            "0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        assert_eq!(
            cart.claims().total,
            Money::parse("0.010000", "USDC").unwrap()
        );
        assert_eq!(cart.claims().total.to_string(), "0.010000 USDC");
        assert_eq!(cart.claims().category, None);
        assert_eq!(cart.claims().line_count, None);
        assert_eq!(cart.attestation().level(), AttestationLevel::AgentReported);
    }

    #[test]
    fn the_hash_is_over_the_document_as_presented() {
        let cart = normalize(&example()).unwrap();
        // Key order does not matter; an unknown field does.
        let reordered: Value = serde_json::from_str(
            r#"{"accepted":{"extra":{"version":"2","name":"USDC"},"maxTimeoutSeconds":60,"payTo":"0x209693Bc6afc0C5328bA36FaF03C514EF312287C","asset":"0x036CbD53842c5426634e7929541eC2318f3dCF7e","amount":"10000","network":"eip155:84532","scheme":"exact"},"resource":{"mimeType":"application/json","description":"Access to premium market data","url":"https://api.example.com/premium-data"},"x402Version":2}"#,
        )
        .unwrap();
        assert_eq!(normalize(&reordered).unwrap().hash(), cart.hash());
        let extended = with(example(), &["accepted", "newField"], json!(1));
        let other = normalize(&extended).unwrap();
        assert_ne!(other.hash(), cart.hash());
        assert_eq!(other.claims(), cart.claims());
    }

    #[test]
    fn base_mainnet_usdc_has_its_own_domain() {
        let mainnet = with(
            with(
                with(example(), &["accepted", "network"], json!("eip155:8453")),
                &["accepted", "asset"],
                json!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            ),
            &["accepted", "extra"],
            json!({"name": "USD Coin", "version": "2"}),
        );
        assert!(normalize(&mainnet).is_ok());
        // The Sepolia domain name on the mainnet token is refused.
        let wrong = with(
            mainnet,
            &["accepted", "extra"],
            json!({"name": "USDC", "version": "2"}),
        );
        assert!(
            matches!(normalize(&wrong), Err(AdapterError::Unsupported(e)) if e.contains("EIP-712"))
        );
    }

    #[test]
    fn amounts_are_exact_in_the_tokens_decimals() {
        let one = with(example(), &["accepted", "amount"], json!("1"));
        assert_eq!(
            normalize(&one).unwrap().claims().total.to_string(),
            "0.000001 USDC"
        );
        let big = with(example(), &["accepted", "amount"], json!("1500000"));
        assert_eq!(
            normalize(&big).unwrap().claims().total.to_string(),
            "1.500000 USDC"
        );
        let zero = with(example(), &["accepted", "amount"], json!("0"));
        assert_eq!(
            normalize(&zero).unwrap().claims().total.to_string(),
            "0.000000 USDC"
        );
        for bad in ["", "-1", "1.5", "1e6", "10,000", " 1", "0x10"] {
            let value = with(example(), &["accepted", "amount"], json!(bad));
            assert!(
                matches!(normalize(&value), Err(AdapterError::Malformed(_))),
                "{bad:?}"
            );
        }
        let huge = with(example(), &["accepted", "amount"], json!("9".repeat(39)));
        assert!(matches!(
            normalize(&huge),
            Err(AdapterError::Unsupported(_))
        ));
        let too_long = with(example(), &["accepted", "amount"], json!("1".repeat(79)));
        assert!(matches!(
            normalize(&too_long),
            Err(AdapterError::Malformed(_))
        ));
    }

    #[test]
    fn only_what_the_adapter_prices_gets_through() {
        let cases: [(&[&str], Value, &str); 12] = [
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
            (&["accepted", "asset"], json!("usdc"), "Malformed"),
            (&["accepted", "payTo"], json!("merchant"), "Malformed"),
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
        ];
        for (path, field, expected) in cases {
            let err = normalize(&with(example(), path, field.clone())).unwrap_err();
            let kind = match err {
                AdapterError::Malformed(_) => "Malformed",
                AdapterError::Unsupported(_) => "Unsupported",
                other => panic!("{path:?}: {other}"),
            };
            assert_eq!(kind, expected, "{path:?} = {field}: {err}");
        }
        let mut no_extra = example();
        no_extra["accepted"]
            .as_object_mut()
            .unwrap()
            .remove("extra");
        assert!(matches!(
            normalize(&no_extra),
            Err(AdapterError::Malformed(_))
        ));
        let mut no_resource = example();
        no_resource.as_object_mut().unwrap().remove("resource");
        assert!(matches!(
            normalize(&no_resource),
            Err(AdapterError::Malformed(_))
        ));
        assert!(matches!(
            adapter().normalize(b"[]"),
            Err(AdapterError::Malformed(_))
        ));
        assert!(matches!(
            adapter().normalize(b"{not json"),
            Err(AdapterError::Malformed(_))
        ));
    }

    #[test]
    fn the_resource_is_checked_as_the_specification_bounds_it() {
        let ok = with(
            example(),
            &["resource"],
            json!({
                "url": "https://api.example.com/premium-data",
                "serviceName": "Example Market Data",
                "tags": ["market-data", "finance"],
                "iconUrl": "https://api.example.com/icon.png"
            }),
        );
        assert!(normalize(&ok).is_ok());
        for (path, field) in [
            (&["resource", "url"][..], json!("")),
            (&["resource", "url"][..], json!("https://a b")),
            (&["resource", "serviceName"][..], json!("x".repeat(33))),
            (&["resource", "serviceName"][..], json!("caf\u{e9}")),
            (
                &["resource", "tags"][..],
                json!(["a", "b", "c", "d", "e", "f"]),
            ),
            (&["resource", "tags"][..], json!([""])),
            (&["resource", "iconUrl"][..], json!("ftp://icons/x.png")),
            (&["resource", "description"][..], json!("line\nbreak")),
        ] {
            let err = normalize(&with(example(), path, field.clone())).unwrap_err();
            assert!(
                matches!(err, AdapterError::Malformed(_)),
                "{path:?} = {field}: {err}"
            );
        }
    }

    #[test]
    fn terms_carry_what_the_rail_will_need() {
        let document: X402Cart = serde_json::from_value(example()).unwrap();
        let terms = adapter().terms(&document.accepted).unwrap();
        assert_eq!(terms.chain, super::super::BASE_SEPOLIA);
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
