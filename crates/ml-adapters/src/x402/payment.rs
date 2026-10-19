//! The x402 v2 `PaymentPayload` for the `exact` scheme on EVM with the
//! default transfer method, EIP-3009: what a client sends to pay.

use super::eip3009::Authorization;
use super::fields::{address, bytes, malformed, uint};
use super::types::{PaymentRequirements, ResourceInfo};
use ml_core::AdapterError;
use serde::{Deserialize, Serialize};

/// A payment: the requirement it pays, and the signed authorization.
///
/// Only `payload.signature` and `payload.authorization` are signed, and
/// only by the payer; `accepted` and `resource` are the client's word.
/// The rail reads the chain and token from `accepted` — a signature made
/// under any other domain does not verify — and everything else that
/// matters from the authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentPayload {
    /// The protocol version. Always 2.
    #[serde(rename = "x402Version")]
    pub x402_version: u8,
    /// What is being bought.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceInfo>,
    /// The requirement this payment meets.
    pub accepted: PaymentRequirements,
    /// The signed authorization.
    pub payload: ExactEvmPayload,
    /// Protocol extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

/// The `payload` of an EIP-3009 payment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactEvmPayload {
    /// The payer's 65-byte signature, hex.
    pub signature: String,
    /// What the payer signed.
    pub authorization: AuthorizationBody,
}

/// EIP-3009's authorization as the wire carries it: every field a string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationBody {
    /// The payer.
    pub from: String,
    /// The payee.
    pub to: String,
    /// The amount, in atomic units.
    pub value: String,
    /// Unix time after which it is valid.
    #[serde(rename = "validAfter")]
    pub valid_after: String,
    /// Unix time before which it is valid.
    #[serde(rename = "validBefore")]
    pub valid_before: String,
    /// The 32-byte nonce, hex.
    pub nonce: String,
}

impl ExactEvmPayload {
    /// The authorization and signature, parsed strictly.
    pub fn parse(&self) -> Result<(Authorization, [u8; 65]), AdapterError> {
        let body = &self.authorization;
        let time = |field: &str, value: &str| {
            u64::try_from(uint(field, value)?)
                .map_err(|_| malformed(&format!("{field}: not a Unix time")))
        };
        let authorization = Authorization {
            from: address("authorization.from", &body.from)?,
            to: address("authorization.to", &body.to)?,
            value: uint("authorization.value", &body.value)?,
            valid_after: time("authorization.validAfter", &body.valid_after)?,
            valid_before: time("authorization.validBefore", &body.valid_before)?,
            nonce: bytes("authorization.nonce", &body.nonce)?,
        };
        let signature = bytes("signature", &self.signature).map_err(|_| {
            malformed(
                "signature: 0x and 130 hex digits; contract-wallet signatures are not accepted",
            )
        })?;
        Ok((authorization, signature))
    }
}
