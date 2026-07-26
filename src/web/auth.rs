//! Access-code auth: stateless HMAC-signed session cookie.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Session cookie name.
pub const COOKIE_NAME: &str = "mh_session";
/// Session lifetime: 30 days.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Current unix time in seconds.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Wall-clock seconds since epoch won't approach i64::MAX for billions of
    // years, so this cast never actually wraps.
    #[allow(clippy::cast_possible_wrap)]
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// HMAC-SHA256(key = `access_code`, msg) → lowercase hex.
/// `new_from_slice` only errors for key-length limits HMAC does not impose, so
/// the error branch is unreachable; return `""` there to stay `unwrap`/`expect`
/// free (the repo denies `unwrap_used`/`expect_used`). An empty signature never
/// verifies, so this is fail-closed.
fn sign(access_code: &str, msg: &str) -> String {
    let Ok(mut mac) = HmacSha256::new_from_slice(access_code.as_bytes()) else {
        return String::new();
    };
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Mint a token `"{exp}.{hex(hmac)}"` where exp = now + TTL.
pub fn mint_token(access_code: &str, now_unix: i64) -> String {
    let exp = now_unix + SESSION_TTL_SECS;
    let exp_str = exp.to_string();
    let sig = sign(access_code, &exp_str);
    format!("{exp_str}.{sig}")
}

/// Verify signature (constant-time) and that the token has not expired.
pub fn verify_token(token: &str, access_code: &str, now_unix: i64) -> bool {
    let Some((exp_str, sig)) = token.split_once('.') else {
        return false;
    };
    if exp_str.is_empty() || sig.is_empty() {
        return false;
    }
    let expected = sign(access_code, exp_str);
    // Constant-time compare of hex signatures.
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    match exp_str.parse::<i64>() {
        Ok(exp) => exp > now_unix,
        Err(_) => false,
    }
}

/// Constant-time comparison of a submitted code against the configured code.
pub fn code_matches(submitted: &str, configured: &str) -> bool {
    submitted
        .as_bytes()
        .ct_eq(configured.as_bytes())
        .unwrap_u8()
        == 1
}

/// `Set-Cookie` value for a fresh session. `secure` adds the Secure attribute
/// (only when the request came over HTTPS, so LAN http still works).
pub fn session_cookie_header(token: &str, secure: bool) -> String {
    let mut h = format!(
        "{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_TTL_SECS}"
    );
    if secure {
        h.push_str("; Secure");
    }
    h
}

/// `Set-Cookie` value that clears the session cookie immediately.
pub fn clear_cookie_header() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

/// Extract the `mh_session` value from a `Cookie` request header.
pub fn token_from_cookie_header(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let pair = pair.trim();
        pair.strip_prefix(concat!("mh_session", "="))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "correct-horse-battery-staple";
    const NOW: i64 = 1_700_000_000;

    #[test]
    fn mint_then_verify_roundtrips() {
        let t = mint_token(CODE, NOW);
        assert!(verify_token(&t, CODE, NOW + 10));
    }

    #[test]
    fn expired_token_is_rejected() {
        let t = mint_token(CODE, NOW);
        assert!(!verify_token(&t, CODE, NOW + SESSION_TTL_SECS + 1));
    }

    #[test]
    fn wrong_code_rejected() {
        let t = mint_token(CODE, NOW);
        assert!(!verify_token(&t, "wrong-code", NOW + 10));
    }

    #[test]
    fn tampered_token_rejected() {
        let t = mint_token(CODE, NOW);
        // Flip the final hex char to a GUARANTEED-different one. (pop+push the
        // same literal char is a no-op when the signature already ends in it.)
        let mut chars: Vec<char> = t.chars().collect();
        if let Some(last) = chars.last_mut() {
            *last = if *last == '0' { '1' } else { '0' };
        }
        let tampered: String = chars.into_iter().collect();
        assert!(!verify_token(&tampered, CODE, NOW + 10));
    }

    #[test]
    fn malformed_token_rejected() {
        assert!(!verify_token("garbage", CODE, NOW));
        assert!(!verify_token("", CODE, NOW));
        assert!(!verify_token("123.", CODE, NOW));
    }

    #[test]
    fn code_matches_is_correct() {
        assert!(code_matches("abc", "abc"));
        assert!(!code_matches("abc", "abd"));
        assert!(!code_matches("abc", "abcd"));
        assert!(!code_matches("", "abc"));
    }

    #[test]
    fn cookie_header_has_expected_attributes() {
        let h = session_cookie_header("tok", true);
        assert!(h.starts_with("mh_session=tok"));
        assert!(h.contains("HttpOnly"));
        assert!(h.contains("SameSite=Lax"));
        assert!(h.contains("Path=/"));
        assert!(h.contains("Max-Age=2592000"));
        assert!(h.contains("Secure"));
        // Insecure (LAN http) must NOT set Secure.
        assert!(!session_cookie_header("tok", false).contains("Secure"));
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        assert!(clear_cookie_header().contains("Max-Age=0"));
    }

    #[test]
    fn extract_token_from_cookie_header() {
        assert_eq!(token_from_cookie_header("mh_session=abc"), Some("abc"));
        assert_eq!(
            token_from_cookie_header("foo=1; mh_session=abc; bar=2"),
            Some("abc")
        );
        assert_eq!(token_from_cookie_header("foo=1; bar=2"), None);
        assert_eq!(token_from_cookie_header(""), None);
    }
}
