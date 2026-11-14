//! Host crypto API: secure randomness, SHA-256, HMAC-SHA256, constant-time compare.
//! Contracts: docs/contracts/ctx-api.md, plans/adr/0019-host-crypto-api.md

use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

pub(crate) const MIN_RANDOM_BYTES: usize = 1;
pub(crate) const MAX_RANDOM_BYTES: usize = 1024;
pub(crate) const MAX_SHA256_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_HMAC_KEY_BYTES: usize = 1024;
pub(crate) const MAX_HMAC_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MIN_TIMING_SAFE_EQUAL_BYTES: usize = 1;
pub(crate) const MAX_TIMING_SAFE_EQUAL_BYTES: usize = 1024;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CryptoError {
    InvalidInput(&'static str),
    RandomUnavailable,
}

impl CryptoError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "crypto_invalid_input",
            Self::RandomUnavailable => "crypto_random_unavailable",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::InvalidInput(msg) => msg,
            Self::RandomUnavailable => "system random entropy unavailable",
        }
    }
}

/// One decoded host crypto operation.
#[derive(PartialEq)]
pub(crate) enum CryptoCall {
    RandomBytes { byte_length: usize },
    Sha256 { input: Vec<u8> },
    HmacSha256 { key: Vec<u8>, message: Vec<u8> },
    TimingSafeEqual { left: Vec<u8>, right: Vec<u8> },
}

/// One successful host crypto result, before wire encoding.
#[derive(PartialEq, Eq)]
pub(crate) enum CryptoReply {
    Bytes(Vec<u8>),
    Equal(bool),
}

pub(crate) trait EntropySource: Send + Sync {
    fn fill(&self, dest: &mut [u8]) -> Result<(), CryptoError>;
}

pub(crate) struct OsEntropy;

impl EntropySource for OsEntropy {
    fn fill(&self, dest: &mut [u8]) -> Result<(), CryptoError> {
        getrandom::fill(dest).map_err(|_| CryptoError::RandomUnavailable)
    }
}

pub(crate) fn sha256_digest(input: &[u8]) -> Result<[u8; 32], CryptoError> {
    if input.len() > MAX_SHA256_INPUT_BYTES {
        return Err(CryptoError::InvalidInput("sha256: input exceeds 8 MiB"));
    }
    let mut hasher = Sha256::new();
    hasher.update(input);
    Ok(hasher.finalize().into())
}

pub(crate) fn hmac_sha256_digest(key: &[u8], message: &[u8]) -> Result<[u8; 32], CryptoError> {
    if key.len() > MAX_HMAC_KEY_BYTES {
        return Err(CryptoError::InvalidInput("hmacSha256: key exceeds 1 KiB"));
    }
    if message.len() > MAX_HMAC_MESSAGE_BYTES {
        return Err(CryptoError::InvalidInput(
            "hmacSha256: message exceeds 8 MiB",
        ));
    }
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| CryptoError::InvalidInput("hmacSha256: invalid key"))?;
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

pub(crate) fn timing_safe_equal(left: &[u8], right: &[u8]) -> Result<bool, CryptoError> {
    if !(MIN_TIMING_SAFE_EQUAL_BYTES..=MAX_TIMING_SAFE_EQUAL_BYTES).contains(&left.len()) {
        return Err(CryptoError::InvalidInput(
            "timingSafeEqual: left length must be in [1, 1024] bytes",
        ));
    }
    if !(MIN_TIMING_SAFE_EQUAL_BYTES..=MAX_TIMING_SAFE_EQUAL_BYTES).contains(&right.len()) {
        return Err(CryptoError::InvalidInput(
            "timingSafeEqual: right length must be in [1, 1024] bytes",
        ));
    }
    if left.len() != right.len() {
        return Ok(false);
    }
    Ok(constant_time_eq::constant_time_eq(left, right))
}

pub(crate) fn generate_random_bytes(
    byte_length: usize,
    entropy: &dyn EntropySource,
) -> Result<Vec<u8>, CryptoError> {
    if !(MIN_RANDOM_BYTES..=MAX_RANDOM_BYTES).contains(&byte_length) {
        return Err(CryptoError::InvalidInput(
            "randomBytes: byteLength must be in [1, 1024]",
        ));
    }
    let mut bytes = vec![0u8; byte_length];
    entropy.fill(&mut bytes)?;
    Ok(bytes)
}

fn decode_crypto_input(
    value: Option<&Json>,
    min_bytes: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, CryptoError> {
    let Some(obj) = value.and_then(Json::as_object) else {
        return Err(CryptoError::InvalidInput("crypto: input must be an object"));
    };
    let Some(kind) = obj.get("type").and_then(Json::as_str) else {
        return Err(CryptoError::InvalidInput("crypto: missing input type"));
    };
    let bytes = match kind {
        "string" => {
            let Some(text) = obj.get("value").and_then(Json::as_str) else {
                return Err(CryptoError::InvalidInput("crypto: invalid string value"));
            };
            text.as_bytes().to_vec()
        }
        "bytes" => {
            let Some(b64) = obj.get("value").and_then(Json::as_str) else {
                return Err(CryptoError::InvalidInput("crypto: invalid bytes value"));
            };
            base64::prelude::BASE64_STANDARD
                .decode(b64)
                .map_err(|_| CryptoError::InvalidInput("crypto: invalid base64 input"))?
        }
        _ => return Err(CryptoError::InvalidInput("crypto: unsupported input type")),
    };
    if bytes.len() < min_bytes || bytes.len() > max_bytes {
        return Err(CryptoError::InvalidInput("crypto: input size out of range"));
    }
    Ok(bytes)
}

/// Decode one bridge payload into a typed crypto operation.
pub(crate) fn parse_crypto_call(payload: &Json) -> Result<CryptoCall, CryptoError> {
    let Some(op) = payload.get("op").and_then(Json::as_str) else {
        return Err(CryptoError::InvalidInput("crypto: missing op"));
    };
    match op {
        "random_bytes" => {
            let Some(byte_length) = payload.get("byte_length").and_then(Json::as_u64) else {
                return Err(CryptoError::InvalidInput(
                    "randomBytes: byteLength must be a positive integer",
                ));
            };
            let Ok(len) = usize::try_from(byte_length) else {
                return Err(CryptoError::InvalidInput(
                    "randomBytes: byteLength out of range",
                ));
            };
            Ok(CryptoCall::RandomBytes { byte_length: len })
        }
        "sha256" => {
            let input = decode_crypto_input(payload.get("input"), 0, MAX_SHA256_INPUT_BYTES)?;
            Ok(CryptoCall::Sha256 { input })
        }
        "hmac_sha256" => {
            let key = decode_crypto_input(payload.get("key"), 0, MAX_HMAC_KEY_BYTES)?;
            let message = decode_crypto_input(payload.get("message"), 0, MAX_HMAC_MESSAGE_BYTES)?;
            Ok(CryptoCall::HmacSha256 { key, message })
        }
        "timing_safe_equal" => {
            let left = decode_crypto_input(
                payload.get("left"),
                MIN_TIMING_SAFE_EQUAL_BYTES,
                MAX_TIMING_SAFE_EQUAL_BYTES,
            )?;
            let right = decode_crypto_input(
                payload.get("right"),
                MIN_TIMING_SAFE_EQUAL_BYTES,
                MAX_TIMING_SAFE_EQUAL_BYTES,
            )?;
            Ok(CryptoCall::TimingSafeEqual { left, right })
        }
        _ => Err(CryptoError::InvalidInput("crypto: unknown operation")),
    }
}

/// Execute one decoded crypto operation with an injectable entropy source.
pub(crate) fn execute_crypto_with_entropy(
    call: CryptoCall,
    entropy: &dyn EntropySource,
) -> Result<CryptoReply, CryptoError> {
    match call {
        CryptoCall::RandomBytes { byte_length } => {
            generate_random_bytes(byte_length, entropy).map(CryptoReply::Bytes)
        }
        CryptoCall::Sha256 { input } => {
            sha256_digest(&input).map(|bytes| CryptoReply::Bytes(bytes.to_vec()))
        }
        CryptoCall::HmacSha256 { key, message } => {
            hmac_sha256_digest(&key, &message).map(|bytes| CryptoReply::Bytes(bytes.to_vec()))
        }
        CryptoCall::TimingSafeEqual { left, right } => {
            timing_safe_equal(&left, &right).map(CryptoReply::Equal)
        }
    }
}

/// Execute one decoded crypto operation with the operating-system entropy source.
pub(crate) fn execute_crypto(call: CryptoCall) -> Result<CryptoReply, CryptoError> {
    execute_crypto_with_entropy(call, &OsEntropy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FailingEntropy;

    impl EntropySource for FailingEntropy {
        fn fill(&self, _dest: &mut [u8]) -> Result<(), CryptoError> {
            Err(CryptoError::RandomUnavailable)
        }
    }

    fn hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    #[test]
    fn sha256_known_answers() {
        // Empty string
        let empty_hash = sha256_digest(b"").expect("empty hash");
        assert_eq!(
            hex(&empty_hash),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // "abc"
        let abc_hash = sha256_digest(b"abc").expect("abc hash");
        assert_eq!(
            hex(&abc_hash),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // Bytes with NUL
        let nul_hash = sha256_digest(b"hello\0world").expect("nul hash");
        assert_eq!(
            hex(&nul_hash),
            "b206899bc103669c8e7b36de29d73f95b46795b508aa87d612b2ce84bfb29df2"
        );
    }

    #[test]
    fn sha256_size_limit() {
        let err = sha256_digest(&vec![0u8; MAX_SHA256_INPUT_BYTES + 1]).expect_err("oversized");
        assert_eq!(
            err,
            CryptoError::InvalidInput("sha256: input exceeds 8 MiB")
        );
    }

    #[test]
    fn hmac_sha256_rfc_4231_vectors() {
        // Test Case 1
        let tc1_key = vec![0x0bu8; 20];
        let tc1_data = b"Hi There";
        let tc1_res = hmac_sha256_digest(&tc1_key, tc1_data).expect("tc1");
        assert_eq!(
            hex(&tc1_res),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );

        // Test Case 2
        let tc2_key = b"Jefe";
        let tc2_data = b"what do ya want for nothing?";
        let tc2_res = hmac_sha256_digest(tc2_key, tc2_data).expect("tc2");
        assert_eq!(
            hex(&tc2_res),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );

        // Test Case 3
        let tc3_key = vec![0xaau8; 20];
        let tc3_data = vec![0xddu8; 50];
        let tc3_res = hmac_sha256_digest(&tc3_key, &tc3_data).expect("tc3");
        assert_eq!(
            hex(&tc3_res),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );

        // Test Case 4
        let tc4_key: Vec<u8> = (0x01..=0x19).collect();
        let tc4_data = vec![0xcdu8; 50];
        let tc4_res = hmac_sha256_digest(&tc4_key, &tc4_data).expect("tc4");
        assert_eq!(
            hex(&tc4_res),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );

        // Test Case 5 (truncation test in RFC, full digest here)
        let tc5_key = vec![0x0cu8; 20];
        let tc5_data = b"Test With Truncation";
        let tc5_res = hmac_sha256_digest(&tc5_key, tc5_data).expect("tc5");
        assert_eq!(
            hex(&tc5_res),
            "a3b6167473100ee06e0c796c2955552bfa6f7c0a6a8aef8b93f860aab0cd20c5"
        );

        // Test Case 6 (key larger than block size: 131 bytes > 64 bytes)
        let tc6_key = vec![0xaau8; 131];
        let tc6_data = b"Test Using Larger Than Block-Size Key - Hash Key First";
        let tc6_res = hmac_sha256_digest(&tc6_key, tc6_data).expect("tc6");
        assert_eq!(
            hex(&tc6_res),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );

        // Test Case 7 (key larger than block size and data larger than block size)
        let tc7_key = vec![0xaau8; 131];
        let tc7_data = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
        let tc7_res = hmac_sha256_digest(&tc7_key, tc7_data).expect("tc7");
        assert_eq!(
            hex(&tc7_res),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
        );
    }

    #[test]
    fn hmac_sha256_size_limits() {
        let key_err = hmac_sha256_digest(&vec![0u8; MAX_HMAC_KEY_BYTES + 1], b"data")
            .expect_err("oversized key");
        assert_eq!(
            key_err,
            CryptoError::InvalidInput("hmacSha256: key exceeds 1 KiB")
        );

        let msg_err = hmac_sha256_digest(b"key", &vec![0u8; MAX_HMAC_MESSAGE_BYTES + 1])
            .expect_err("oversized message");
        assert_eq!(
            msg_err,
            CryptoError::InvalidInput("hmacSha256: message exceeds 8 MiB")
        );
    }

    #[test]
    fn timing_safe_equal_semantics() {
        // Equal
        assert!(timing_safe_equal(b"secret_key_1234", b"secret_key_1234").expect("equal"));
        // Not equal same length
        assert!(!timing_safe_equal(b"secret_key_1234", b"secret_key_5678").expect("unequal"));
        // Differing length returns false without error
        assert!(!timing_safe_equal(b"short", b"longer_string").expect("diff length"));
        // Empty inputs are rejected
        assert_eq!(
            timing_safe_equal(b"", b"valid").expect_err("empty left"),
            CryptoError::InvalidInput("timingSafeEqual: left length must be in [1, 1024] bytes")
        );
        assert_eq!(
            timing_safe_equal(b"valid", b"").expect_err("empty right"),
            CryptoError::InvalidInput("timingSafeEqual: right length must be in [1, 1024] bytes")
        );
        // Oversized inputs are rejected
        let oversized = vec![0u8; MAX_TIMING_SAFE_EQUAL_BYTES + 1];
        assert_eq!(
            timing_safe_equal(&oversized, b"valid").expect_err("oversized left"),
            CryptoError::InvalidInput("timingSafeEqual: left length must be in [1, 1024] bytes")
        );
    }

    #[test]
    fn random_bytes_generation_and_limits() {
        // Min length 1
        let one = generate_random_bytes(1, &OsEntropy).expect("1 byte");
        assert_eq!(one.len(), 1);

        // Max length 1024
        let max = generate_random_bytes(1024, &OsEntropy).expect("1024 bytes");
        assert_eq!(max.len(), 1024);

        // Two distinct calls produce distinct values
        let a = generate_random_bytes(32, &OsEntropy).expect("call a");
        let b = generate_random_bytes(32, &OsEntropy).expect("call b");
        assert_ne!(a, b);

        // 0 rejected
        assert_eq!(
            generate_random_bytes(0, &OsEntropy).expect_err("0 len"),
            CryptoError::InvalidInput("randomBytes: byteLength must be in [1, 1024]")
        );

        // 1025 rejected
        assert_eq!(
            generate_random_bytes(1025, &OsEntropy).expect_err("1025 len"),
            CryptoError::InvalidInput("randomBytes: byteLength must be in [1, 1024]")
        );

        // Injectable entropy failure path
        assert_eq!(
            generate_random_bytes(32, &FailingEntropy).expect_err("failing entropy"),
            CryptoError::RandomUnavailable
        );
    }

    #[test]
    fn crypto_bridge_parses_and_executes_typed_calls() {
        // random_bytes
        let call =
            parse_crypto_call(&json!({ "op": "random_bytes", "byte_length": 16 })).expect("parse");
        let CryptoReply::Bytes(bytes) =
            execute_crypto_with_entropy(call, &OsEntropy).expect("random bytes")
        else {
            panic!("random_bytes returned a non-bytes reply");
        };
        assert_eq!(bytes.len(), 16);

        // random_bytes failure injection
        let call =
            parse_crypto_call(&json!({ "op": "random_bytes", "byte_length": 16 })).expect("parse");
        let error = execute_crypto_with_entropy(call, &FailingEntropy)
            .err()
            .expect("entropy failure");
        assert_eq!(error, CryptoError::RandomUnavailable,);

        // sha256 string
        let call = parse_crypto_call(&json!({
            "op": "sha256",
            "input": { "type": "string", "value": "abc" }
        }))
        .expect("parse sha256");
        let CryptoReply::Bytes(digest) = execute_crypto(call).expect("sha256") else {
            panic!("sha256 returned a non-bytes reply");
        };
        assert_eq!(
            hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // timing_safe_equal
        let call = parse_crypto_call(&json!({
            "op": "timing_safe_equal",
            "left": { "type": "string", "value": "tokenA" },
            "right": { "type": "string", "value": "tokenA" }
        }))
        .expect("parse timing_safe_equal");
        assert!(matches!(execute_crypto(call), Ok(CryptoReply::Equal(true))));
    }
}
