use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng as PwOsRng, PasswordHash, PasswordHasher, SaltString},
    Argon2, PasswordVerifier,
};
use axum::http::{header, HeaderMap, HeaderValue};
use parking_lot::Mutex;
use rand::{rngs::OsRng, RngCore};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SESSION_COOKIE: &str = "vs_session";
pub const SHORT_SESSION_TTL_SECS: i64 = 60 * 60 * 24; // 24h
pub const LONG_SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30; // 30d
pub const MIN_PASSWORD_LEN: usize = 6;
pub const MIN_USERNAME_LEN: usize = 3;
pub const MAX_USERNAME_LEN: usize = 24;

pub fn validate_username(s: &str) -> Result<String, &'static str> {
    let trimmed = s.trim();
    if trimmed.len() < MIN_USERNAME_LEN || trimmed.len() > MAX_USERNAME_LEN {
        return Err("username_invalid");
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("username_invalid");
    }
    Ok(trimmed.to_string())
}

pub fn validate_password(s: &str) -> Result<(), &'static str> {
    if s.chars().count() < MIN_PASSWORD_LEN {
        return Err("password_too_short");
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut PwOsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn gen_session_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(SESSION_COOKIE) {
            if let Some(value) = rest.strip_prefix('=') {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Cookie attributes that vary by deployment:
/// - HTTPS (prod): `SameSite=None; Secure; Partitioned` so the cookie works in
///   cross-site iframes (e.g. embedded on itch.io). Partitioned opts into CHIPS,
///   so each top-level site gets its own session jar.
/// - HTTP (dev): `SameSite=Lax` (Secure cookies are rejected over plain http,
///   and we don't try to support iframes in dev).
fn cookie_site_attrs(request_headers: &HeaderMap) -> &'static str {
    if forwarded_https(request_headers) {
        "; SameSite=None; Secure; Partitioned"
    } else {
        "; SameSite=Lax"
    }
}

pub fn set_session_cookie(token: &str, max_age_secs: i64, request_headers: &HeaderMap) -> HeaderValue {
    let attrs = cookie_site_attrs(request_headers);
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; Max-Age={max_age_secs}{attrs}"
    ))
    .expect("cookie value ascii")
}

pub fn clear_session_cookie(request_headers: &HeaderMap) -> HeaderValue {
    let attrs = cookie_site_attrs(request_headers);
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; Max-Age=0{attrs}"
    ))
    .expect("cookie value ascii")
}

pub fn client_ip(headers: &HeaderMap, peer: IpAddr) -> IpAddr {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    peer
}

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Returns true if this attempt is allowed. Records the attempt on success.
    pub fn allow(&self, ip: IpAddr, limit: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut g = self.buckets.lock();
        let bucket = g.entry(ip).or_default();
        while let Some(&t) = bucket.front() {
            if now.duration_since(t) > window {
                bucket.pop_front();
            } else {
                break;
            }
        }
        if bucket.len() >= limit {
            return false;
        }
        bucket.push_back(now);
        true
    }
}
