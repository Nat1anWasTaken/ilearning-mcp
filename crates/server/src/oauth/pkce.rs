//! PKCE (RFC 7636), `S256` only.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// An `S256` challenge is the unpadded base64url encoding of a SHA-256 digest.
pub fn is_valid_challenge(challenge: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(challenge)
        .is_ok_and(|digest| digest.len() == 32)
}

pub fn verify(verifier: &str, challenge: &str) -> bool {
    let well_formed = (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b));
    well_formed && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)) == challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    // CHALLENGE = base64url(sha256(VERIFIER)), computed independently with openssl.
    const VERIFIER: &str = "dBjftJeZ4CVP-mJ92K9-OInbo0-2G2SVvBNLgSsxNjk";
    const CHALLENGE: &str = "vkRMquJVBhilqWKo7Eh-mZY-FzEJuYL95XVg6gH_Qpk";

    #[test]
    fn verifies_matching_pair() {
        assert!(is_valid_challenge(CHALLENGE));
        assert!(verify(VERIFIER, CHALLENGE));
    }

    #[test]
    fn rejects_mismatches_and_malformed_input() {
        assert!(!verify(&VERIFIER.replace('d', "e"), CHALLENGE));
        assert!(!verify(VERIFIER, &CHALLENGE.replace('v', "w")));
        assert!(!verify("short", CHALLENGE));
        assert!(!verify(&"a".repeat(129), CHALLENGE));
        assert!(!is_valid_challenge("plain-verifier"));
        assert!(!is_valid_challenge(&format!("{CHALLENGE}=")));
    }
}
