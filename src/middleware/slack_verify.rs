use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Verify a Slack request signature.
///
/// Slack sends three values for request verification:
/// - `X-Slack-Request-Timestamp` header
/// - `X-Slack-Signature` header (format: "v0=hex_digest")
/// - The raw request body
///
/// The expected signature is HMAC-SHA256 of "v0:{timestamp}:{body}" using the signing secret,
/// prefixed with "v0=".
///
/// Returns `true` if the signature is valid.
pub fn verify_slack_signature(
    signing_secret: &str,
    timestamp: &str,
    body: &str,
    signature: &str,
) -> bool {
    // Build the base string: "v0:{timestamp}:{body}"
    let sig_basestring = format!("v0:{timestamp}:{body}");

    // Compute HMAC-SHA256
    let mut mac = match HmacSha256::new_from_slice(signing_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(sig_basestring.as_bytes());
    let result = mac.finalize();
    let computed_hex = hex::encode(result.into_bytes());
    let computed_signature = format!("v0={computed_hex}");

    // Constant-time comparison to prevent timing attacks
    let expected = computed_signature.as_bytes();
    let provided = signature.as_bytes();

    expected.len() == provided.len() && expected.ct_eq(provided).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_valid_signature() {
        let secret = "8f742231b10e8888abcd99yez56789d0";
        let timestamp = "1531420618";
        let body = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c659f2945b5b60";

        // Compute the expected signature for the test
        let sig_basestring = format!("v0:{timestamp}:{body}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(sig_basestring.as_bytes());
        let result = mac.finalize();
        let expected_sig = format!("v0={}", hex::encode(result.into_bytes()));

        assert!(verify_slack_signature(secret, timestamp, body, &expected_sig));
    }

    #[test]
    fn test_verify_invalid_signature() {
        let secret = "test-secret";
        let timestamp = "1234567890";
        let body = "test body";

        assert!(!verify_slack_signature(
            secret,
            timestamp,
            body,
            "v0=0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn test_verify_wrong_format() {
        let secret = "test-secret";
        let timestamp = "1234567890";
        let body = "test body";

        assert!(!verify_slack_signature(secret, timestamp, body, "invalid"));
    }
}
