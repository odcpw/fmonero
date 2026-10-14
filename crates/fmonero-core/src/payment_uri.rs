//! Pure, bounded Monero payment requests. Parsing never contacts or mutates a wallet.
use crate::{address, Error, NetType, Piconero};
use serde::{Deserialize, Serialize};

pub const MAX_URI_BYTES: usize = 8192;
pub const MAX_TEXT_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub address: String,
    pub amount: Option<Piconero>,
    pub description: Option<String>,
    pub recipient_name: Option<String>,
}

fn bad() -> Error {
    Error::usage("This payment request has invalid, repeated or unsupported fields.")
        .with_reason("bad_args")
        .with_hint("fmonero wallet validate <address>")
}

fn validate(request: &PaymentRequest, network: NetType) -> Result<(), Error> {
    if address::parse(&request.address)?.net != network {
        return Err(Error::invalid_address(
            "The payment request belongs to another Monero network.",
        ));
    }
    if request.amount == Some(Piconero(0)) {
        return Err(Error::usage(
            "Omit the amount for an open request, or enter an amount greater than zero.",
        ));
    }
    for text in [&request.description, &request.recipient_name]
        .into_iter()
        .flatten()
    {
        if text.len() > MAX_TEXT_BYTES || text.chars().any(char::is_control) {
            return Err(bad());
        }
    }
    Ok(())
}

pub fn parse(uri: &str, network: NetType) -> Result<PaymentRequest, Error> {
    if uri.len() > MAX_URI_BYTES || uri.contains('#') || uri.chars().any(char::is_control) {
        return Err(bad());
    }
    let body = uri.strip_prefix("monero:").ok_or_else(bad)?;
    let (address, query) = body
        .split_once('?')
        .map_or((body, None), |(a, q)| (a, Some(q)));
    let mut request = PaymentRequest {
        address: address.to_owned(),
        amount: None,
        description: None,
        recipient_name: None,
    };
    let mut keys = std::collections::BTreeSet::new();
    if let Some(query) = query {
        if query.is_empty() {
            return Err(bad());
        }
        for parameter in query.split('&') {
            let (key, value) = parameter.split_once('=').ok_or_else(bad)?;
            if !keys.insert(key) {
                return Err(bad());
            }
            let value = decode(value)?;
            match key {
                "tx_amount" => {
                    if value.ends_with('p') || value.is_empty() {
                        return Err(bad());
                    }
                    request.amount = Some(Piconero::parse(&value).map_err(|_| bad())?);
                }
                "tx_description" => request.description = Some(value),
                "recipient_name" => request.recipient_name = Some(value),
                _ => return Err(bad()),
            }
        }
    }
    validate(&request, network)?;
    Ok(request)
}

pub fn format(request: &PaymentRequest, network: NetType) -> Result<String, Error> {
    validate(request, network)?;
    let mut params = Vec::new();
    if let Some(amount) = request.amount {
        params.push(format!("tx_amount={}", amount.to_xmr_string()));
    }
    if let Some(text) = &request.description {
        params.push(format!("tx_description={}", encode(text)));
    }
    if let Some(text) = &request.recipient_name {
        params.push(format!("recipient_name={}", encode(text)));
    }
    let mut uri = format!("monero:{}", request.address);
    if !params.is_empty() {
        uri.push('?');
        uri.push_str(&params.join("&"));
    }
    if uri.len() > MAX_URI_BYTES {
        return Err(bad());
    }
    Ok(uri)
}

fn decode(input: &str) -> Result<String, Error> {
    let mut out = Vec::with_capacity(input.len().min(MAX_TEXT_BYTES));
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if out.len() >= MAX_TEXT_BYTES {
            return Err(bad());
        }
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1).ok_or_else(bad)?;
            let lo = *bytes.get(i + 2).ok_or_else(bad)?;
            let digit = |b: u8| (b as char).to_digit(16).map(|n| n as u8).ok_or_else(bad);
            out.push(digit(hi)? * 16 + digit(lo)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).map_err(|_| bad())?;
    if text.chars().any(char::is_control) {
        return Err(bad());
    }
    Ok(text)
}

fn encode(text: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const ADDRESS: &str = "888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H";
    #[test]
    fn request_roundtrip_preserves_piconero_and_unicode() {
        for amount in [None, Some(Piconero(1)), Some(Piconero(u64::MAX))] {
            let request = PaymentRequest {
                address: ADDRESS.into(),
                amount,
                description: Some("Café + 茶 & 50%".into()),
                recipient_name: Some("A=B".into()),
            };
            let uri = format(&request, NetType::Mainnet).unwrap();
            assert_eq!(parse(&uri, NetType::Mainnet).unwrap(), request);
        }
    }
    #[test]
    fn unsafe_requests_fail_without_any_backend() {
        for query in [
            "tx_amount=1&tx_amount=2",
            "payment_id=01",
            "other=1",
            "tx_amount=1e2",
            "tx_amount=1p",
            "tx_amount=-1",
            "tx_amount=0",
            "tx_description=%",
            "tx_description=%FF",
            "tx_description=%1B%5D52",
            "tx_amount=18446744073709551616",
            "tx_description=x#fragment",
        ] {
            assert!(
                parse(&format!("monero:{ADDRESS}?{query}"), NetType::Mainnet).is_err(),
                "{query}"
            );
        }
        assert!(parse(&format!("monero:{ADDRESS}"), NetType::Testnet).is_err());
    }
}
