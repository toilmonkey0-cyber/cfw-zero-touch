//! Optional Gumroad license for signed builds and support.
//!
//! Backup, restore, probes, full checks, guided setup, and the disk refusals
//! do not consult this module. The on-disk cache is a speed bump: this app is
//! open source, so a determined person can skip the check. See
//! `docs/LICENSING.md`.

#![forbid(unsafe_code)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const VERIFY_URL: &str = "https://api.gumroad.com/v2/licenses/verify";
pub const PLACEHOLDER_PRODUCT_ID: &str = "REPLACE_WITH_GUMROAD_PRODUCT_ID";
pub const SOFT_LIMIT: u64 = 3;
pub const REVERIFY_AFTER_SECS: i64 = 7 * 24 * 60 * 60;
pub const GRACE_PERIOD_SECS: i64 = 90 * 24 * 60 * 60;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: usize = 64 * 1024;
const CACHE_VERSION: u32 = 1;

const MSG_ACTIVE: &str = "License active on this computer.";
const MSG_SOFT: &str = "This license is already in use on more than 3 computers. Card Studio keeps working on this one. If that was a surprise, contact support and we'll help.";
const MSG_STALE: &str =
    "Couldn't reach Gumroad to re-check this license. This computer stays activated.";
const MSG_STALE_GRACE: &str = "Couldn't reach Gumroad to re-check this license. It has been over 90 days since the last check, and this computer stays activated.";
const MSG_STALE_UNREADABLE: &str =
    "Gumroad returned a response this app could not read. This computer stays activated.";
const MSG_ENTER: &str =
    "Enter the license key from your Gumroad receipt to activate this computer.";
const MSG_UNRECOGNIZED: &str =
    "That license key was not recognized. Check the key from your Gumroad receipt and try again.";
const MSG_REFUNDED: &str = "This license was refunded, so it can't be used.";
const MSG_CHARGEBACKED: &str = "This license was charged back, so it can't be used.";
const MSG_DISPUTED: &str = "This license is in a payment dispute, so it can't be used.";
const MSG_PRODUCT_MISMATCH: &str =
    "Gumroad's response didn't match this app's product, so the license was not accepted.";
const MSG_MALFORMED: &str =
    "That doesn't look like a license key. Paste the key from your Gumroad receipt.";
const MSG_NO_PRODUCT: &str = "This build has no Gumroad product id yet. Card tools still work.";
const MSG_NO_MACHINE: &str = "This computer didn't provide a machine id, so a license can't be saved for offline use. Card tools still work.";
const MSG_UNTRUSTED: &str =
    "The saved license can't be used on this computer. Enter your license key again.";
const MSG_OFFLINE: &str = "Couldn't reach Gumroad to check this license. Try again when you're online. Card tools still work.";
const MSG_UNREADABLE: &str =
    "Gumroad returned a response this app could not read. Nothing was changed. Try again.";
const MSG_STORE: &str =
    "The license checked out, but it could not be saved on this computer. Try again.";
const MSG_NEEDS_LICENSE: &str = "Signed builds and support need a license. Backup, restore, card tests, and disk safeguards stay available.";
const MSG_UNCHANGED_SUFFIX: &str = "The license already on this computer is unchanged.";

/// Build-time product id. Unsigned and source builds keep the placeholder.
pub fn configured_product_id() -> &'static str {
    match option_env!("CFW_GUMROAD_PRODUCT_ID") {
        Some(id) if !id.is_empty() => id,
        _ => PLACEHOLDER_PRODUCT_ID,
    }
}

pub fn cache_path() -> PathBuf {
    crate::store::store_root()
        .join("license")
        .join("activation.json")
}

pub fn session_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseState {
    Active,
    Inactive,
}

impl LicenseState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Active,
    NotActivated,
    Unrecognized,
    Refunded,
    Chargebacked,
    Disputed,
    ProductMismatch,
    MalformedKey,
    ProductUnconfigured,
    NoMachine,
    UntrustedCache,
    Unreachable,
    Unreadable,
    StoreFailed,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NotActivated => "not_activated",
            Self::Unrecognized => "unrecognized",
            Self::Refunded => "refunded",
            Self::Chargebacked => "chargebacked",
            Self::Disputed => "disputed",
            Self::ProductMismatch => "product_mismatch",
            Self::MalformedKey => "malformed_key",
            Self::ProductUnconfigured => "product_unconfigured",
            Self::NoMachine => "no_machine",
            Self::UntrustedCache => "untrusted_cache",
            Self::Unreachable => "unreachable",
            Self::Unreadable => "unreadable",
            Self::StoreFailed => "store_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseStatus {
    pub state: LicenseState,
    pub reason: Reason,
    pub message: String,
    pub uses: Option<u64>,
    pub over_soft_limit: bool,
    pub stale: bool,
    pub network_kind: Option<&'static str>,
}

impl LicenseStatus {
    pub fn log_detail(&self) -> String {
        format!(
            "state={} reason={} uses={} over_soft_limit={} stale={} network={}",
            self.state.as_str(),
            self.reason.as_str(),
            self.uses.unwrap_or(0),
            self.over_soft_limit,
            self.stale,
            self.network_kind.unwrap_or("none"),
        )
    }

    pub fn to_view(&self, notice: Option<&str>) -> LicenseView {
        let message = match notice {
            Some(notice) if self.state == LicenseState::Active => {
                format!("{notice} {MSG_UNCHANGED_SUFFIX}")
            }
            Some(notice) => notice.to_string(),
            None => self.message.clone(),
        };
        LicenseView {
            state: self.state.as_str().to_string(),
            reason: self.reason.as_str().to_string(),
            message,
            uses: self.uses,
            over_soft_limit: self.over_soft_limit,
            stale: self.stale,
            signed_updates: matches!(gate(Feature::SignedUpdates, self), Gate::Open),
            support: matches!(gate(Feature::Support, self), Gate::Open),
            safety_open: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateOutcome {
    pub stored: LicenseStatus,
    pub notice: Option<String>,
}

impl ActivateOutcome {
    pub fn to_view(&self) -> LicenseView {
        self.stored.to_view(self.notice.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseView {
    pub state: String,
    pub reason: String,
    pub message: String,
    pub uses: Option<u64>,
    pub over_soft_limit: bool,
    pub stale: bool,
    pub signed_updates: bool,
    pub support: bool,
    pub safety_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Backup,
    Restore,
    Probe,
    FullCheck,
    DiskRefusal,
    GuidedSetup,
    SignedUpdates,
    Support,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Open,
    NeedsLicense { message: String },
}

pub fn is_safety(feature: Feature) -> bool {
    matches!(
        feature,
        Feature::Backup
            | Feature::Restore
            | Feature::Probe
            | Feature::FullCheck
            | Feature::DiskRefusal
            | Feature::GuidedSetup
    )
}

/// Safety features are open with or without a license. Above the 3-computer
/// soft limit, signed updates and support stay open too.
pub fn gate(feature: Feature, status: &LicenseStatus) -> Gate {
    if is_safety(feature) || status.state == LicenseState::Active {
        Gate::Open
    } else {
        Gate::NeedsLicense {
            message: MSG_NEEDS_LICENSE.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct VerifyCall {
    pub url: String,
    pub product_id: String,
    pub license_key: String,
    pub increment_uses_count: bool,
    pub timeout: Duration,
}

impl std::fmt::Debug for VerifyCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifyCall")
            .field("url", &self.url)
            .field("product_id", &self.product_id)
            .field("license_key", &redact_license_key())
            .field("increment_uses_count", &self.increment_uses_count)
            .field("timeout", &self.timeout)
            .finish()
    }
}

pub struct RawReply {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Debug for RawReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawReply")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportError {
    pub kind: &'static str,
}

pub trait LicenseTransport {
    fn verify(&self, call: &VerifyCall) -> Result<RawReply, TransportError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpPolicy {
    pub https_only: bool,
    pub redirects: u32,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub proxy_from_env: bool,
}

pub fn http_policy(timeout: Duration) -> HttpPolicy {
    HttpPolicy {
        https_only: true,
        redirects: 0,
        timeout,
        connect_timeout: CONNECT_TIMEOUT,
        proxy_from_env: false,
    }
}

pub fn redact_license_key() -> &'static str {
    "****"
}

/// SHA-256 of a domain separator plus the raw machine material.
pub fn machine_id_hash(material: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cfw-card-studio-machine-v1");
    hasher.update([0]);
    hasher.update(material.as_bytes());
    hex_encode(&hasher.finalize())
}

pub fn local_machine_material() -> Result<String, String> {
    read_local_machine_material()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UreqTransport;

impl LicenseTransport for UreqTransport {
    fn verify(&self, call: &VerifyCall) -> Result<RawReply, TransportError> {
        post_verify(call)
    }
}

pub struct Session<T: LicenseTransport> {
    transport: T,
    cache_path: PathBuf,
    product_id: String,
    verify_url: String,
    machine_material: String,
    timeout: Duration,
}

impl<T: LicenseTransport> Session<T> {
    pub fn new(
        transport: T,
        cache_path: PathBuf,
        product_id: impl Into<String>,
        machine_material: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            transport,
            cache_path,
            product_id: product_id.into(),
            verify_url: VERIFY_URL.to_string(),
            machine_material: machine_material.into(),
            timeout,
        }
    }

    pub fn status(&self, now: i64) -> LicenseStatus {
        match self.load() {
            CacheLoad::Missing => inactive(Reason::NotActivated, MSG_ENTER),
            CacheLoad::Untrusted => inactive(Reason::UntrustedCache, MSG_UNTRUSTED),
            CacheLoad::Trusted(cached) => self.reconcile(cached, now).stored,
        }
    }

    pub fn activate(&self, license_key: &str, now: i64) -> ActivateOutcome {
        let key = license_key.trim();
        if let Some(failure) = self.preflight(key) {
            return self.outcome_keeping_cache(failure);
        }
        match self.load() {
            CacheLoad::Trusted(cached) if cached.license_key == key => self.reconcile(cached, now),
            _ => self.activate_new(key, now),
        }
    }

    fn preflight(&self, key: &str) -> Option<LicenseStatus> {
        if !key_is_well_formed(key) {
            return Some(inactive(Reason::MalformedKey, MSG_MALFORMED));
        }
        if !product_id_configured(&self.product_id) {
            return Some(inactive(Reason::ProductUnconfigured, MSG_NO_PRODUCT));
        }
        if self.machine_material.trim().is_empty() {
            return Some(inactive(Reason::NoMachine, MSG_NO_MACHINE));
        }
        if !https_verify_url(&self.verify_url) {
            return Some(inactive(Reason::Unreachable, MSG_OFFLINE));
        }
        None
    }

    fn activate_new(&self, key: &str, now: i64) -> ActivateOutcome {
        if let Err(failure) = self.verify(key, false) {
            return self.failure_outcome(failure);
        }
        let verified = match self.verify(key, true) {
            Ok(verified) => verified,
            Err(failure) => return self.failure_outcome(failure),
        };
        let stored = Stored {
            license_key: key.to_string(),
            uses: verified.uses,
            verified_at: now,
        };
        if let Err(status) = self.store(&stored) {
            return ActivateOutcome {
                stored: status,
                notice: None,
            };
        }
        ActivateOutcome {
            stored: status_from_stored(&stored),
            notice: None,
        }
    }

    fn reconcile(&self, cached: Stored, now: i64) -> ActivateOutcome {
        if !needs_reverify(cached.verified_at, now) {
            return ActivateOutcome {
                stored: status_from_stored(&cached),
                notice: None,
            };
        }
        match self.verify(&cached.license_key, false) {
            Ok(verified) => {
                let updated = Stored {
                    license_key: cached.license_key,
                    uses: verified.uses,
                    verified_at: now,
                };
                if let Err(status) = self.store(&updated) {
                    return ActivateOutcome {
                        stored: status,
                        notice: None,
                    };
                }
                ActivateOutcome {
                    stored: status_from_stored(&updated),
                    notice: None,
                }
            }
            Err(Failure::Rejected(reason)) => {
                self.discard_cache();
                ActivateOutcome {
                    stored: inactive(reason, message_for(reason)),
                    notice: None,
                }
            }
            Err(Failure::Unreachable(kind)) => ActivateOutcome {
                stored: stale_status(&cached, now, Reason::Unreachable, kind, false),
                notice: None,
            },
            Err(Failure::Unreadable) => ActivateOutcome {
                stored: stale_status(&cached, now, Reason::Unreadable, "unreadable", true),
                notice: None,
            },
        }
    }

    fn failure_outcome(&self, failure: Failure) -> ActivateOutcome {
        self.outcome_keeping_cache(failure_status(failure))
    }

    fn outcome_keeping_cache(&self, failure: LicenseStatus) -> ActivateOutcome {
        match self.load() {
            CacheLoad::Trusted(cached) => ActivateOutcome {
                stored: status_from_stored(&cached),
                notice: Some(failure.message),
            },
            _ => ActivateOutcome {
                stored: failure,
                notice: None,
            },
        }
    }

    fn verify(&self, key: &str, increment: bool) -> Result<Verified, Failure> {
        let call = VerifyCall {
            url: self.verify_url.clone(),
            product_id: self.product_id.clone(),
            license_key: key.to_string(),
            increment_uses_count: increment,
            timeout: self.timeout,
        };
        if call.url != VERIFY_URL || !https_verify_url(&call.url) {
            return Err(Failure::Unreachable("insecure"));
        }
        match self.transport.verify(&call) {
            Ok(reply) => interpret(&reply.body, reply.status, &self.product_id),
            Err(error) => Err(Failure::Unreachable(error.kind)),
        }
    }

    fn store(&self, stored: &Stored) -> Result<(), LicenseStatus> {
        let payload = Payload {
            v: CACHE_VERSION,
            product_id: self.product_id.clone(),
            license_key: stored.license_key.clone(),
            machine_id: self.machine_hash(),
            uses: stored.uses,
            verified_at: stored.verified_at,
        };
        write_cache(&self.cache_path, &payload)
            .map_err(|_| inactive(Reason::StoreFailed, MSG_STORE))
    }

    fn load(&self) -> CacheLoad {
        let machine_hash = self.machine_hash();
        match read_cache(&self.cache_path, &machine_hash, &self.product_id) {
            Ok(Some(payload)) => CacheLoad::Trusted(Stored {
                license_key: payload.license_key,
                uses: payload.uses,
                verified_at: payload.verified_at,
            }),
            Ok(None) => CacheLoad::Missing,
            Err(()) => CacheLoad::Untrusted,
        }
    }

    fn machine_hash(&self) -> String {
        machine_id_hash(self.machine_material.trim())
    }

    fn discard_cache(&self) {
        if fs::remove_file(&self.cache_path).is_ok() {
            return;
        }
        // If the file cannot be removed, replace it so the next launch does
        // not trust the previous activation. The replacement has no key.
        let _ = fs::write(&self.cache_path, b"revoked");
    }
}

pub fn production_session() -> Session<UreqTransport> {
    Session::new(
        UreqTransport,
        cache_path(),
        configured_product_id(),
        local_machine_material().unwrap_or_default(),
        REQUEST_TIMEOUT,
    )
}

struct Stored {
    license_key: String,
    uses: u64,
    verified_at: i64,
}

enum CacheLoad {
    Missing,
    Untrusted,
    Trusted(Stored),
}

struct Verified {
    uses: u64,
}

enum Failure {
    Rejected(Reason),
    Unreachable(&'static str),
    Unreadable,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Payload {
    v: u32,
    product_id: String,
    license_key: String,
    machine_id: String,
    uses: u64,
    verified_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Envelope {
    payload: Payload,
    mac: String,
}

fn inactive(reason: Reason, message: &str) -> LicenseStatus {
    LicenseStatus {
        state: LicenseState::Inactive,
        reason,
        message: message.to_string(),
        uses: None,
        over_soft_limit: false,
        stale: false,
        network_kind: None,
    }
}

fn status_from_stored(stored: &Stored) -> LicenseStatus {
    let over = stored.uses > SOFT_LIMIT;
    LicenseStatus {
        state: LicenseState::Active,
        reason: Reason::Active,
        message: active_message(over, false, false, false),
        uses: Some(stored.uses),
        over_soft_limit: over,
        stale: false,
        network_kind: None,
    }
}

fn stale_status(
    stored: &Stored,
    now: i64,
    reason: Reason,
    kind: &'static str,
    unreadable: bool,
) -> LicenseStatus {
    let over = stored.uses > SOFT_LIMIT;
    LicenseStatus {
        state: LicenseState::Active,
        reason,
        message: active_message(over, true, past_grace(stored.verified_at, now), unreadable),
        uses: Some(stored.uses),
        over_soft_limit: over,
        stale: true,
        network_kind: Some(kind),
    }
}

fn active_message(over: bool, stale: bool, grace: bool, unreadable: bool) -> String {
    let mut text = if over {
        MSG_SOFT.to_string()
    } else {
        MSG_ACTIVE.to_string()
    };
    if stale {
        text.push(' ');
        text.push_str(if unreadable {
            MSG_STALE_UNREADABLE
        } else if grace {
            MSG_STALE_GRACE
        } else {
            MSG_STALE
        });
    }
    text
}

fn message_for(reason: Reason) -> &'static str {
    match reason {
        Reason::Active => MSG_ACTIVE,
        Reason::NotActivated => MSG_ENTER,
        Reason::Unrecognized => MSG_UNRECOGNIZED,
        Reason::Refunded => MSG_REFUNDED,
        Reason::Chargebacked => MSG_CHARGEBACKED,
        Reason::Disputed => MSG_DISPUTED,
        Reason::ProductMismatch => MSG_PRODUCT_MISMATCH,
        Reason::MalformedKey => MSG_MALFORMED,
        Reason::ProductUnconfigured => MSG_NO_PRODUCT,
        Reason::NoMachine => MSG_NO_MACHINE,
        Reason::UntrustedCache => MSG_UNTRUSTED,
        Reason::Unreachable => MSG_OFFLINE,
        Reason::Unreadable => MSG_UNREADABLE,
        Reason::StoreFailed => MSG_STORE,
    }
}

fn failure_status(failure: Failure) -> LicenseStatus {
    match failure {
        Failure::Rejected(reason) => inactive(reason, message_for(reason)),
        Failure::Unreachable(kind) => {
            let mut status = inactive(Reason::Unreachable, MSG_OFFLINE);
            status.network_kind = Some(kind);
            status
        }
        Failure::Unreadable => inactive(Reason::Unreadable, MSG_UNREADABLE),
    }
}

fn needs_reverify(verified_at: i64, now: i64) -> bool {
    if now < verified_at {
        return true;
    }
    now.saturating_sub(verified_at) >= REVERIFY_AFTER_SECS
}

fn past_grace(verified_at: i64, now: i64) -> bool {
    now.saturating_sub(verified_at) >= GRACE_PERIOD_SECS
}

fn key_is_well_formed(key: &str) -> bool {
    (8..=128).contains(&key.len())
        && key.bytes().any(|byte| byte.is_ascii_alphanumeric())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn product_id_configured(id: &str) -> bool {
    id != PLACEHOLDER_PRODUCT_ID
        && (8..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'='))
}

fn https_verify_url(url: &str) -> bool {
    url == VERIFY_URL
        && url.starts_with("https://")
        && !url.contains('@')
        && !url.bytes().any(|byte| byte.is_ascii_whitespace())
}

fn interpret(body: &str, status: u16, expected_product: &str) -> Result<Verified, Failure> {
    if !(200..300).contains(&status) && !matches!(status, 400 | 404 | 422) {
        return Err(Failure::Unreachable("http"));
    }
    if body.len() > MAX_BODY {
        return Err(Failure::Unreadable);
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| Failure::Unreadable)?;
    let success = value
        .get("success")
        .and_then(|item| item.as_bool())
        .ok_or(Failure::Unreadable)?;
    if !success {
        return Err(Failure::Rejected(Reason::Unrecognized));
    }
    let purchase = value.get("purchase").ok_or(Failure::Unreadable)?;
    let product_id = purchase
        .get("product_id")
        .and_then(|item| item.as_str())
        .ok_or(Failure::Unreadable)?;
    if product_id != expected_product {
        return Err(Failure::Rejected(Reason::ProductMismatch));
    }
    let refunded = flag(purchase, "refunded")?;
    let chargebacked = flag(purchase, "chargebacked")?;
    let disputed = flag(purchase, "disputed")?;
    if refunded {
        return Err(Failure::Rejected(Reason::Refunded));
    }
    if chargebacked {
        return Err(Failure::Rejected(Reason::Chargebacked));
    }
    if disputed {
        return Err(Failure::Rejected(Reason::Disputed));
    }
    let uses = value
        .get("uses")
        .and_then(|item| item.as_u64())
        .ok_or(Failure::Unreadable)?;
    Ok(Verified { uses })
}

fn flag(purchase: &serde_json::Value, name: &str) -> Result<bool, Failure> {
    purchase
        .get(name)
        .and_then(|item| item.as_bool())
        .ok_or(Failure::Unreadable)
}

fn form_body(product_id: &str, license_key: &str, increment: bool) -> String {
    let flag = if increment { "true" } else { "false" };
    format!(
        "product_id={}&license_key={}&increment_uses_count={flag}",
        urlencoded(product_id),
        urlencoded(license_key),
    )
}

fn urlencoded(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn post_verify(call: &VerifyCall) -> Result<RawReply, TransportError> {
    if call.url != VERIFY_URL || !https_verify_url(&call.url) {
        return Err(TransportError { kind: "insecure" });
    }
    let policy = http_policy(call.timeout);
    let agent = build_agent(&policy);
    // The body is exactly these three fields. There is no API token.
    let body = form_body(
        &call.product_id,
        &call.license_key,
        call.increment_uses_count,
    );
    let result = agent
        .post(&call.url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);
    match result {
        Ok(response) => read_reply(response),
        Err(ureq::Error::Status(status, response)) => {
            if status >= 500 || (300..400).contains(&status) {
                let _ = read_limited(response);
                return Err(TransportError {
                    kind: if status >= 500 {
                        "http_5xx"
                    } else {
                        "redirect"
                    },
                });
            }
            read_reply(response)
        }
        Err(ureq::Error::Transport(transport)) => Err(TransportError {
            kind: transport_kind(transport.kind()),
        }),
    }
}

fn build_agent(policy: &HttpPolicy) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(policy.https_only)
        .redirects(policy.redirects)
        .timeout(policy.timeout)
        .timeout_connect(policy.connect_timeout)
        .try_proxy_from_env(policy.proxy_from_env)
        .build()
}

fn read_reply(response: ureq::Response) -> Result<RawReply, TransportError> {
    let status = response.status();
    if status >= 500 {
        let _ = read_limited(response);
        return Err(TransportError { kind: "http_5xx" });
    }
    if (300..400).contains(&status) {
        let _ = read_limited(response);
        return Err(TransportError { kind: "redirect" });
    }
    Ok(RawReply {
        status,
        body: read_limited(response)?,
    })
}

fn read_limited(response: ureq::Response) -> Result<String, TransportError> {
    let mut buf = Vec::new();
    response
        .into_reader()
        .take((MAX_BODY as u64).saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|_| TransportError { kind: "read" })?;
    if buf.len() > MAX_BODY {
        return Err(TransportError { kind: "read" });
    }
    String::from_utf8(buf).map_err(|_| TransportError { kind: "read" })
}

fn transport_kind(kind: ureq::ErrorKind) -> &'static str {
    match kind {
        ureq::ErrorKind::Dns => "dns",
        ureq::ErrorKind::ConnectionFailed => "connection",
        ureq::ErrorKind::TooManyRedirects => "redirect",
        ureq::ErrorKind::InvalidUrl
        | ureq::ErrorKind::UnknownScheme
        | ureq::ErrorKind::InsecureRequestHttpsOnly => "insecure",
        ureq::ErrorKind::Io => "io",
        _ => "transport",
    }
}

fn write_cache(path: &Path, payload: &Payload) -> Result<(), ()> {
    let bytes = serde_json::to_vec(payload).map_err(|_| ())?;
    let mac = sign(&payload.machine_id, &bytes);
    let envelope = Envelope {
        payload: Payload {
            v: payload.v,
            product_id: payload.product_id.clone(),
            license_key: payload.license_key.clone(),
            machine_id: payload.machine_id.clone(),
            uses: payload.uses,
            verified_at: payload.verified_at,
        },
        mac,
    };
    let file_bytes = serde_json::to_vec_pretty(&envelope).map_err(|_| ())?;
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(|_| ())?;
        file.write_all(&file_bytes).map_err(|_| ())?;
        let _ = file.sync_all();
        #[cfg(unix)]
        restrict_file(&file);
    }
    fs::rename(&tmp, path).map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn read_cache(path: &Path, machine_hash: &str, product_id: &str) -> Result<Option<Payload>, ()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let payload_bytes = serde_json::to_vec(&envelope.payload).map_err(|_| ())?;
    if !mac_ok(machine_hash, &payload_bytes, &envelope.mac) {
        return Err(());
    }
    if envelope.payload.v != CACHE_VERSION
        || envelope.payload.product_id != product_id
        || envelope.payload.machine_id != machine_hash
        || !key_is_well_formed(&envelope.payload.license_key)
    {
        return Err(());
    }
    Ok(Some(envelope.payload))
}

fn create_private_dir(path: &Path) -> Result<(), ()> {
    fs::create_dir_all(path).map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_file(file: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
}

fn mac_key(machine_hash: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"cfw-card-studio-license-mac-v1");
    hasher.update([0]);
    hasher.update(machine_hash.as_bytes());
    hasher.finalize().into()
}

fn sign(machine_hash: &str, payload: &[u8]) -> String {
    let key = mac_key(machine_hash);
    let mut mac = HmacSha256::new_from_slice(&key).expect("hmac accepts a 32-byte key");
    mac.update(payload);
    hex_encode(&mac.finalize().into_bytes())
}

fn mac_ok(machine_hash: &str, payload: &[u8], mac_hex: &str) -> bool {
    let Some(bytes) = hex_decode(mac_hex) else {
        return false;
    };
    let key = mac_key(machine_hash);
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return false;
    };
    mac.update(payload);
    mac.verify_slice(&bytes).is_ok()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let hi = hex_val(bytes[index])?;
        let lo = hex_val(bytes[index + 1])?;
        out.push((hi << 4) | lo);
        index += 2;
    }
    Some(out)
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(any(target_os = "windows", test))]
fn parse_machine_guid(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("MachineGuid") else {
            continue;
        };
        let Some(guid) = rest.split_whitespace().last() else {
            continue;
        };
        let guid = guid.trim_matches(|ch| ch == '{' || ch == '}');
        if (8..=64).contains(&guid.len())
            && guid.bytes().any(|byte| byte.is_ascii_hexdigit())
            && guid
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Some(guid.to_string());
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_local_machine_material() -> Result<String, String> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(text) = fs::read_to_string(path) {
            let id = text.trim();
            if (8..=128).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Ok(id.to_string());
            }
        }
    }
    Err("machine id unavailable".into())
}

#[cfg(target_os = "windows")]
fn read_local_machine_material() -> Result<String, String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .map_err(|_| "machine id unavailable".to_string())?;
    if !output.status.success() {
        return Err("machine id unavailable".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_machine_guid(&text).ok_or_else(|| "machine id unavailable".into())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn read_local_machine_material() -> Result<String, String> {
    Err("machine id unavailable".into())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    const KEY: &str = "ABCD-EFGH-IJKL-MN99";
    const OTHER_KEY: &str = "ZZZZ-YYYY-XXXX-WW99";
    const ECHO: &str = "ECHO-KEY-SHOULD-NOT-LEAK-9999";
    const EMAIL: &str = "buyer@example.invalid";
    const PRODUCT: &str = "fixture-product-id-001";
    const MACHINE: &str = "test-machine-a";
    const NOW: i64 = 1_700_000_000;

    struct Script {
        calls: Arc<Mutex<Vec<VerifyCall>>>,
        replies: Mutex<VecDeque<Result<RawReply, TransportError>>>,
    }

    impl Script {
        fn new(replies: Vec<Result<RawReply, TransportError>>) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                replies: Mutex::new(replies.into()),
            }
        }
    }

    impl LicenseTransport for Script {
        fn verify(&self, call: &VerifyCall) -> Result<RawReply, TransportError> {
            self.calls.lock().expect("call log").push(call.clone());
            self.replies
                .lock()
                .expect("script")
                .pop_front()
                .unwrap_or(Err(TransportError {
                    kind: "no_scripted_reply",
                }))
        }
    }

    struct Lab {
        session: Session<Script>,
        calls: Arc<Mutex<Vec<VerifyCall>>>,
        dir: PathBuf,
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn scratch() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cfw-license-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lab_with(
        machine: &str,
        product: &str,
        replies: Vec<Result<RawReply, TransportError>>,
    ) -> Lab {
        let dir = scratch();
        let script = Script::new(replies);
        let calls = Arc::clone(&script.calls);
        let session = Session::new(
            script,
            dir.join("license").join("activation.json"),
            product,
            machine,
            REQUEST_TIMEOUT,
        );
        Lab {
            session,
            calls,
            dir,
        }
    }

    fn lab(replies: Vec<Result<RawReply, TransportError>>) -> Lab {
        lab_with(MACHINE, PRODUCT, replies)
    }

    fn calls_of(lab: &Lab) -> Vec<VerifyCall> {
        lab.calls.lock().expect("calls").clone()
    }

    fn recorded(name: &str) -> &'static str {
        match name {
            "valid.json" => include_str!("../tests/fixtures/gumroad/valid.json"),
            "refunded.json" => include_str!("../tests/fixtures/gumroad/refunded.json"),
            "chargebacked.json" => include_str!("../tests/fixtures/gumroad/chargebacked.json"),
            "disputed.json" => include_str!("../tests/fixtures/gumroad/disputed.json"),
            "dispute-won.json" => include_str!("../tests/fixtures/gumroad/dispute-won.json"),
            "unrecognized.json" => include_str!("../tests/fixtures/gumroad/unrecognized.json"),
            other => panic!("unknown fixture {other}"),
        }
    }

    fn reply_uses(uses: u64) -> Result<RawReply, TransportError> {
        let mut value: serde_json::Value = serde_json::from_str(recorded("valid.json")).unwrap();
        value["uses"] = serde_json::json!(uses);
        Ok(RawReply {
            status: 200,
            body: serde_json::to_string(&value).unwrap(),
        })
    }

    fn reply_file(name: &str, status: u16) -> Result<RawReply, TransportError> {
        Ok(RawReply {
            status,
            body: recorded(name).to_string(),
        })
    }

    fn assert_secret_free(text: &str) {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains(OTHER_KEY), "{text}");
        assert!(!text.contains(ECHO), "{text}");
        assert!(!text.contains(EMAIL), "{text}");
    }

    fn cache_file(lab: &Lab) -> PathBuf {
        lab.dir.join("license").join("activation.json")
    }

    #[test]
    fn refunded_chargebacked_and_disputed_keys_are_rejected() {
        for (name, reason, message) in [
            ("refunded.json", Reason::Refunded, MSG_REFUNDED),
            ("chargebacked.json", Reason::Chargebacked, MSG_CHARGEBACKED),
            ("disputed.json", Reason::Disputed, MSG_DISPUTED),
        ] {
            let lab = lab(vec![reply_file(name, 200)]);
            let outcome = lab.session.activate(KEY, NOW);
            assert_eq!(outcome.stored.state, LicenseState::Inactive);
            assert_eq!(outcome.stored.reason, reason);
            assert_eq!(outcome.stored.message, message);
            assert_secret_free(&outcome.stored.message);
            assert_secret_free(&outcome.stored.log_detail());
            assert!(!cache_file(&lab).exists());
            assert_eq!(calls_of(&lab).len(), 1);
            assert!(!calls_of(&lab)[0].increment_uses_count);
        }
    }

    #[test]
    fn a_dispute_the_seller_won_is_accepted_only_when_disputed_is_false() {
        let lab = lab(vec![reply_file("dispute-won.json", 200), reply_uses(1)]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.state, LicenseState::Active);
        assert!(!outcome.stored.over_soft_limit);
    }

    #[test]
    fn an_unrecognized_key_uses_plain_language_and_hides_the_key() {
        let lab = lab(vec![reply_file("unrecognized.json", 404)]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::Unrecognized);
        assert_eq!(outcome.stored.message, MSG_UNRECOGNIZED);
        assert_secret_free(&outcome.stored.message);
        assert_secret_free(&outcome.to_view().message);
        assert!(!cache_file(&lab).exists());
    }

    #[test]
    fn a_new_activation_checks_without_increment_then_increments_once() {
        let lab = lab(vec![reply_uses(2), reply_uses(3)]);
        let padded = format!("  {KEY}  ");
        let outcome = lab.session.activate(&padded, NOW);
        assert_eq!(outcome.stored.state, LicenseState::Active);
        assert_eq!(outcome.stored.uses, Some(3));
        assert!(!outcome.stored.over_soft_limit);
        assert_eq!(outcome.stored.message, MSG_ACTIVE);
        let calls = calls_of(&lab);
        assert_eq!(calls.len(), 2);
        assert!(!calls[0].increment_uses_count);
        assert!(calls[1].increment_uses_count);
        assert!(calls.iter().all(|call| call.license_key == KEY));
        assert!(calls.iter().all(|call| call.url == VERIFY_URL));
        assert!(calls.iter().all(|call| call.product_id == PRODUCT));
        assert!(calls.iter().all(|call| call.timeout == REQUEST_TIMEOUT));
        let file = fs::read_to_string(cache_file(&lab)).unwrap();
        assert!(file.contains(KEY));
        assert!(!file.contains(ECHO));
        assert!(!file.contains(EMAIL));
        assert_secret_free(&format!("{:?}", calls[0]));
    }

    #[test]
    fn launching_again_inside_the_window_does_not_increment_or_call_out() {
        let lab = lab(vec![reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW);
        let before = calls_of(&lab).len();
        let status = lab.session.status(NOW + REVERIFY_AFTER_SECS - 1);
        assert_eq!(status.state, LicenseState::Active);
        assert!(!status.stale);
        assert_eq!(calls_of(&lab).len(), before);
        let again = lab.session.activate(KEY, NOW + 10);
        assert_eq!(again.stored.state, LicenseState::Active);
        assert_eq!(calls_of(&lab).len(), before);
    }

    #[test]
    fn offline_launch_works_after_one_activation() {
        let lab = lab(vec![reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW);
        let status = lab.session.status(NOW + 60);
        assert_eq!(status.state, LicenseState::Active);
        assert_eq!(status.reason, Reason::Active);
        assert!(!status.stale);
    }

    #[test]
    fn a_copied_or_edited_cache_is_not_accepted_and_is_not_sent() {
        let lab = lab(vec![reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW);
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(cache_file(&lab)).unwrap()).unwrap();
        value["payload"]["uses"] = serde_json::json!(99);
        fs::write(cache_file(&lab), serde_json::to_vec(&value).unwrap()).unwrap();
        let before = calls_of(&lab).len();
        let status = lab.session.status(NOW + 5);
        assert_eq!(status.state, LicenseState::Inactive);
        assert_eq!(status.reason, Reason::UntrustedCache);
        assert_eq!(calls_of(&lab).len(), before);
        assert_secret_free(&status.message);
    }

    #[test]
    fn another_computers_cache_does_not_activate_this_one() {
        let origin = lab_with("machine-a", PRODUCT, vec![reply_uses(1), reply_uses(1)]);
        origin.session.activate(KEY, NOW);
        let other = lab_with("machine-b", PRODUCT, vec![]);
        fs::create_dir_all(cache_file(&other).parent().unwrap()).unwrap();
        fs::copy(cache_file(&origin), cache_file(&other)).unwrap();
        let status = other.session.status(NOW);
        assert_eq!(status.state, LicenseState::Inactive);
        assert_eq!(status.reason, Reason::UntrustedCache);
        assert!(calls_of(&other).is_empty());
    }

    #[test]
    fn reverify_does_not_increment_and_a_network_failure_keeps_the_cache() {
        let lab = lab(vec![
            reply_uses(1),
            reply_uses(1),
            Err(TransportError { kind: "dns" }),
        ]);
        lab.session.activate(KEY, NOW);
        let status = lab.session.status(NOW + REVERIFY_AFTER_SECS);
        assert_eq!(status.state, LicenseState::Active);
        assert!(status.stale);
        assert_eq!(status.reason, Reason::Unreachable);
        assert_eq!(status.network_kind, Some("dns"));
        assert!(status.message.contains("stays activated"));
        assert!(!status.message.contains("90 days"));
        assert_secret_free(&status.message);
        let calls = calls_of(&lab);
        assert_eq!(calls.len(), 3);
        assert!(!calls[2].increment_uses_count);
        assert!(cache_file(&lab).exists());
    }

    #[test]
    fn a_long_outage_still_does_not_revoke_a_cached_license() {
        let lab = lab(vec![
            reply_uses(1),
            reply_uses(1),
            Err(TransportError { kind: "timeout" }),
        ]);
        lab.session.activate(KEY, NOW);
        let status = lab.session.status(NOW + GRACE_PERIOD_SECS);
        assert_eq!(status.state, LicenseState::Active);
        assert!(status.stale);
        assert!(status.message.contains("90 days"));
        assert!(status.message.contains("stays activated"));
        assert_secret_free(&status.message);
    }

    #[test]
    fn a_successful_reverify_refreshes_the_cache_without_incrementing() {
        let lab = lab(vec![reply_uses(1), reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW);
        let status = lab.session.status(NOW + REVERIFY_AFTER_SECS);
        assert_eq!(status.state, LicenseState::Active);
        assert!(!status.stale);
        assert!(!calls_of(&lab)[2].increment_uses_count);
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(cache_file(&lab)).unwrap()).unwrap();
        assert_eq!(
            value["payload"]["verified_at"],
            serde_json::json!(NOW + REVERIFY_AFTER_SECS)
        );
    }

    #[test]
    fn a_refund_discovered_on_reverify_removes_the_cache() {
        let lab = lab(vec![
            reply_uses(1),
            reply_uses(1),
            reply_file("refunded.json", 200),
        ]);
        lab.session.activate(KEY, NOW);
        let status = lab.session.status(NOW + REVERIFY_AFTER_SECS);
        assert_eq!(status.reason, Reason::Refunded);
        assert!(!cache_file(&lab).exists());
        assert_secret_free(&status.message);
    }

    #[test]
    fn http_500_and_redirects_and_html_do_not_revoke_or_echo_the_body() {
        let html = format!("<html>{KEY} {ECHO}</html>");
        for (label, reply) in [
            (
                "status 500",
                Ok(RawReply {
                    status: 500,
                    body: html.clone(),
                }),
            ),
            (
                "redirect",
                Ok(RawReply {
                    status: 302,
                    body: html.clone(),
                }),
            ),
            (
                "html 404",
                Ok(RawReply {
                    status: 404,
                    body: html.clone(),
                }),
            ),
        ] {
            let lab = lab(vec![reply_uses(1), reply_uses(1), reply]);
            lab.session.activate(KEY, NOW);
            let status = lab.session.status(NOW + REVERIFY_AFTER_SECS);
            assert_eq!(status.state, LicenseState::Active, "{label}");
            assert!(status.stale, "{label}");
            assert!(cache_file(&lab).exists(), "{label}");
            assert_secret_free(&status.message);
            assert_secret_free(&status.log_detail());
        }
    }

    #[test]
    fn the_fourth_computer_is_warned_and_still_activates() {
        let lab = lab(vec![reply_uses(3), reply_uses(4)]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.state, LicenseState::Active);
        assert!(outcome.stored.over_soft_limit);
        assert_eq!(outcome.stored.uses, Some(4));
        assert!(outcome.stored.message.contains("more than 3"));
        assert!(outcome.stored.message.contains("keeps working"));
        assert!(matches!(
            gate(Feature::SignedUpdates, &outcome.stored),
            Gate::Open
        ));
        assert!(matches!(
            gate(Feature::Support, &outcome.stored),
            Gate::Open
        ));
    }

    #[test]
    fn three_uses_is_within_the_soft_limit() {
        let lab = lab(vec![reply_uses(2), reply_uses(3)]);
        let outcome = lab.session.activate(KEY, NOW);
        assert!(!outcome.stored.over_soft_limit);
        assert_eq!(outcome.stored.message, MSG_ACTIVE);
    }

    #[test]
    fn a_failed_new_key_does_not_replace_a_saved_license() {
        let lab = lab(vec![
            reply_uses(1),
            reply_uses(1),
            reply_file("unrecognized.json", 404),
        ]);
        lab.session.activate(KEY, NOW);
        let outcome = lab.session.activate(OTHER_KEY, NOW + 20);
        assert_eq!(outcome.stored.state, LicenseState::Active);
        assert_eq!(outcome.stored.uses, Some(1));
        let notice = outcome.notice.clone().expect("failure notice");
        assert!(notice.contains("not recognized"));
        assert_secret_free(&notice);
        let view = outcome.to_view();
        assert!(view.message.contains(MSG_UNCHANGED_SUFFIX));
        assert!(view.signed_updates);
        assert!(view.safety_open);
        assert_secret_free(&view.message);
        let file = fs::read_to_string(cache_file(&lab)).unwrap();
        assert!(file.contains(KEY));
        assert!(!file.contains(OTHER_KEY));
        assert_eq!(calls_of(&lab).len(), 3);
        assert!(!calls_of(&lab)[2].increment_uses_count);
    }

    #[test]
    fn placeholder_product_id_and_bad_keys_never_call_the_network() {
        let placeholder = lab_with(MACHINE, PLACEHOLDER_PRODUCT_ID, vec![]);
        let outcome = placeholder.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::ProductUnconfigured);
        assert!(calls_of(&placeholder).is_empty());
        assert_secret_free(&outcome.stored.message);

        let lab = lab(vec![]);
        for bad in ["", "abc", "bad key", "----", "has space"] {
            let outcome = lab.session.activate(bad, NOW);
            assert_eq!(outcome.stored.reason, Reason::MalformedKey, "{bad}");
        }
        assert!(calls_of(&lab).is_empty());

        let no_machine = lab_with("  ", PRODUCT, vec![]);
        let outcome = no_machine.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::NoMachine);
        assert!(calls_of(&no_machine).is_empty());
    }

    #[test]
    fn a_missing_purchase_flag_is_not_treated_as_false() {
        let mut value: serde_json::Value = serde_json::from_str(recorded("valid.json")).unwrap();
        value["purchase"]
            .as_object_mut()
            .unwrap()
            .remove("chargebacked");
        let lab = lab(vec![Ok(RawReply {
            status: 200,
            body: serde_json::to_string(&value).unwrap(),
        })]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::Unreadable);
        assert!(!cache_file(&lab).exists());
        assert_secret_free(&outcome.stored.message);
    }

    #[test]
    fn a_response_for_a_different_product_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(recorded("valid.json")).unwrap();
        value["purchase"]["product_id"] = serde_json::json!("some-other-product");
        let lab = lab(vec![Ok(RawReply {
            status: 200,
            body: serde_json::to_string(&value).unwrap(),
        })]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::ProductMismatch);
        assert!(!cache_file(&lab).exists());
    }

    #[test]
    fn the_second_call_failing_does_not_save_a_half_activation() {
        let lab = lab(vec![
            reply_uses(1),
            Err(TransportError { kind: "connection" }),
        ]);
        let outcome = lab.session.activate(KEY, NOW);
        assert_eq!(outcome.stored.state, LicenseState::Inactive);
        assert_eq!(outcome.stored.reason, Reason::Unreachable);
        assert!(outcome.stored.message.contains("Card tools still work"));
        assert!(!cache_file(&lab).exists());
        assert_eq!(calls_of(&lab).len(), 2);
    }

    #[test]
    fn safety_features_and_disk_refusals_work_with_no_license() {
        let inactive = inactive(Reason::NotActivated, MSG_ENTER);
        let active = status_from_stored(&Stored {
            license_key: KEY.to_string(),
            uses: 9,
            verified_at: NOW,
        });
        assert!(active.over_soft_limit);
        for feature in [
            Feature::Backup,
            Feature::Restore,
            Feature::Probe,
            Feature::FullCheck,
            Feature::DiskRefusal,
            Feature::GuidedSetup,
        ] {
            assert_eq!(gate(feature, &inactive), Gate::Open);
            assert_eq!(gate(feature, &active), Gate::Open);
            assert!(is_safety(feature));
        }
        assert!(matches!(
            gate(Feature::SignedUpdates, &inactive),
            Gate::NeedsLicense { .. }
        ));
        assert!(matches!(
            gate(Feature::Support, &inactive),
            Gate::NeedsLicense { .. }
        ));
        assert!(matches!(gate(Feature::SignedUpdates, &active), Gate::Open));
        let view = inactive.to_view(None);
        assert!(view.safety_open);
        assert!(!view.signed_updates);
        assert!(!view.support);

        let card = |removable: bool, bytes: u64| crate::volume::VolumeInfo {
            id: "E:".into(),
            letter: "E".into(),
            label: "UNTITLED".into(),
            file_system: "NTFS".into(),
            total_bytes: bytes,
            is_removable: removable,
            is_empty: false,
        };
        let request = |bytes: u64| crate::format_gate::FormatRequest {
            volume_id: "E:".into(),
            letter: "E".into(),
            displayed_bytes: bytes,
            confirmation: "FORMAT".into(),
            file_system: "exFAT".into(),
            label: "EASYROMS".into(),
        };
        let ready = request(32_000_000_000);
        crate::format_gate::authorize_format(&card(true, 32_000_000_000), &ready).unwrap();
        assert_eq!(
            crate::format_gate::authorize_format(&card(false, 32_000_000_000), &ready).unwrap_err(),
            crate::format_gate::FormatBlock::NotRemovable
        );
        let zero = request(0);
        assert_eq!(
            crate::format_gate::authorize_format(&card(true, 0), &zero).unwrap_err(),
            crate::format_gate::FormatBlock::Size
        );

        let disk = crate::flash::FlashDisk {
            number: 1,
            bus: "USB".into(),
            size_bytes: 100,
            system_disk: false,
        };
        let plan = crate::flash::FlashPlan {
            confirmation: "FLASH".into(),
            disk_number: 1,
            displayed_bytes: 100,
            expected_sha256: "abc".into(),
            actual_sha256: "abc".into(),
        };
        crate::flash::authorize_flash(&disk, &plan).unwrap();
        let system = crate::flash::FlashDisk {
            number: 0,
            bus: "USB".into(),
            size_bytes: 100,
            system_disk: false,
        };
        let system_plan = crate::flash::FlashPlan {
            disk_number: 0,
            ..plan
        };
        assert_eq!(
            crate::flash::authorize_flash(&system, &system_plan).unwrap_err(),
            crate::flash::FlashBlock::NotRemovable
        );
        let error = crate::read_disk_identity(0).unwrap_err();
        assert!(error.contains("disk 0"), "{error}");
    }

    #[test]
    fn safety_commands_do_not_consult_the_license_module() {
        let src = include_str!("lib.rs");
        for name in [
            "backup_stock_card",
            "cancel_stock_backup",
            "restore_backup",
            "cancel_restore",
            "probe_card",
            "cancel_probe",
            "full_check_card",
            "cancel_full_check",
            "flash_os",
            "prepare_card",
            "list_volumes",
            "list_flash_disks",
            "list_probe_disks",
            "firstboot_state",
            "read_disk_identity",
        ] {
            let body = command_body(src, name);
            assert!(
                !body.contains("license::"),
                "{name} consults the license module"
            );
        }
    }

    fn command_body<'a>(src: &'a str, name: &str) -> &'a str {
        let marker = format!("fn {name}(");
        let start = src
            .find(&marker)
            .unwrap_or_else(|| panic!("missing {name}"));
        let rest = &src[start..];
        let next_fn = rest.find("\nfn ").unwrap_or(rest.len());
        let next_command = rest.find("\n#[tauri::command]").unwrap_or(rest.len());
        &rest[..next_fn.min(next_command)]
    }

    #[test]
    fn http_policy_is_https_only_with_no_redirects_and_a_timeout() {
        let policy = http_policy(REQUEST_TIMEOUT);
        assert!(policy.https_only);
        assert_eq!(policy.redirects, 0);
        assert!(!policy.proxy_from_env);
        assert_eq!(policy.timeout, Duration::from_secs(15));
        assert_eq!(policy.connect_timeout, Duration::from_secs(5));
        assert_eq!(VERIFY_URL, "https://api.gumroad.com/v2/licenses/verify");
        assert!(https_verify_url(VERIFY_URL));
        assert!(!https_verify_url(
            "http://api.gumroad.com/v2/licenses/verify"
        ));
        assert!(!https_verify_url(
            "https://example.invalid/v2/licenses/verify"
        ));
        assert!(!https_verify_url(
            "https://user:pass@api.gumroad.com/v2/licenses/verify"
        ));
    }

    #[test]
    fn the_ureq_transport_refuses_any_url_other_than_gumroad_verify() {
        let transport = UreqTransport;
        for url in [
            "http://127.0.0.1:9/v2/licenses/verify",
            "https://example.invalid/v2/licenses/verify",
            "https://api.gumroad.com/v2/licenses/verify.evil",
        ] {
            let error = transport
                .verify(&VerifyCall {
                    url: url.to_string(),
                    product_id: PRODUCT.to_string(),
                    license_key: KEY.to_string(),
                    increment_uses_count: false,
                    timeout: REQUEST_TIMEOUT,
                })
                .unwrap_err();
            assert_eq!(error.kind, "insecure", "{url}");
        }
    }

    #[test]
    fn form_body_is_three_fields_and_encodes_the_product_id() {
        let body = form_body("ab==", KEY, false);
        assert_eq!(
            body,
            "product_id=ab%3D%3D&license_key=ABCD-EFGH-IJKL-MN99&increment_uses_count=false"
        );
        assert!(!body.contains("access_token"));
        assert!(!body.contains("api_key"));
        let enabled = form_body(PRODUCT, KEY, true);
        assert!(enabled.ends_with("increment_uses_count=true"));
    }

    #[test]
    fn debug_output_redacts_the_key_and_the_response_body() {
        let call = VerifyCall {
            url: VERIFY_URL.to_string(),
            product_id: PRODUCT.to_string(),
            license_key: KEY.to_string(),
            increment_uses_count: false,
            timeout: REQUEST_TIMEOUT,
        };
        let rendered = format!("{call:?}");
        assert_secret_free(&rendered);
        assert!(rendered.contains("****"));
        let reply = RawReply {
            status: 200,
            body: recorded("valid.json").to_string(),
        };
        assert_secret_free(&format!("{reply:?}"));
    }

    #[test]
    fn machine_id_hash_is_stable_and_not_the_raw_material() {
        assert_eq!(
            machine_id_hash("abc"),
            "5e6d653c25d808a1b708c45af34f1184ec11ae835313f76db195bae007545459"
        );
        assert_ne!(machine_id_hash("abc"), machine_id_hash("abd"));
        let material = local_machine_material().expect("machine id");
        let hash = machine_id_hash(&material);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(hash, material);
        assert_eq!(hash, machine_id_hash(&material));
    }

    #[test]
    fn windows_machine_guid_parser_reads_the_registry_line() {
        let sample = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    MachineGuid    REG_SZ    {12345678-1234-1234-1234-123456789abc}\r\n";
        assert_eq!(
            parse_machine_guid(sample).as_deref(),
            Some("12345678-1234-1234-1234-123456789abc")
        );
        assert_eq!(parse_machine_guid("no guid here"), None);
    }

    #[test]
    fn hmac_sha256_matches_the_rfc_vector() {
        let mut mac = HmacSha256::new_from_slice(&[0x0b; 20]).unwrap();
        mac.update(b"Hi There");
        assert_eq!(
            hex_encode(&mac.finalize().into_bytes()),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn the_default_product_id_is_a_placeholder_and_the_module_has_no_token() {
        assert_eq!(PLACEHOLDER_PRODUCT_ID, "REPLACE_WITH_GUMROAD_PRODUCT_ID");
        if option_env!("CFW_GUMROAD_PRODUCT_ID").is_none() {
            assert_eq!(configured_product_id(), PLACEHOLDER_PRODUCT_ID);
        }
        assert!(!product_id_configured(PLACEHOLDER_PRODUCT_ID));
        let src = include_str!("license.rs");
        let prod = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!prod.contains("access_token"));
        assert!(!prod.contains("api_key"));
        assert!(!prod.contains("Authorization"));
        assert!(!prod.contains("diag::"));
    }

    #[cfg(unix)]
    #[test]
    fn the_cache_file_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;
        let lab = lab(vec![reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW);
        let file = cache_file(&lab);
        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(file.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_clock_stamp_in_the_future_is_rechecked() {
        let lab = lab(vec![reply_uses(1), reply_uses(1), reply_uses(1)]);
        lab.session.activate(KEY, NOW + 86_400);
        let status = lab.session.status(NOW);
        assert_eq!(status.state, LicenseState::Active);
        assert!(!status.stale);
        assert_eq!(calls_of(&lab).len(), 3);
        assert!(!calls_of(&lab)[2].increment_uses_count);
    }

    #[test]
    fn saving_fails_closed_when_the_cache_directory_cannot_be_created() {
        let dir = scratch();
        let blocker = dir.join("not-a-directory");
        fs::write(&blocker, b"file").unwrap();
        let script = Script::new(vec![reply_uses(1), reply_uses(1)]);
        let calls = Arc::clone(&script.calls);
        let session = Session::new(
            script,
            blocker.join("activation.json"),
            PRODUCT,
            MACHINE,
            REQUEST_TIMEOUT,
        );
        let outcome = session.activate(KEY, NOW);
        assert_eq!(outcome.stored.reason, Reason::StoreFailed);
        assert_eq!(outcome.stored.state, LicenseState::Inactive);
        assert_secret_free(&outcome.stored.message);
        assert_eq!(calls.lock().unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
