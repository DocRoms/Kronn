//! KT-1492 — the strong confirmation that raises a trust level.
//!
//! Raising a level (to Trusted or Full access, at any scope), and changing or
//! disabling this confirmation itself, needs a proof from a factor that cannot
//! be driven through the UI: a TOTP code (`core::totp`) or one of ten
//! single-use recovery codes. Lowering or revoking never needs it, except for
//! a bridge-token caller, which always presents a code.
//!
//! The code is the proof, whatever the channel ([`Channel`]): the UI, an
//! `X-Kronn-Confirm` header or a body field over HTTP, or a bridge-token call
//! (the owner may give the code to an agent for that one action). The guards
//! are the same on every channel: a code is single use (a replay is refused),
//! it is consumed by the one route and object of the call that presented it
//! ([`Binding`], recorded), and every use and refusal is journalled with the
//! channel.
//!
//! **Verify-and-consume is one critical section.** A process-wide mutex
//! ([`exclusive`]) covers every read-modify-write of the record, and the record
//! is replaced by an atomic rename: two simultaneous requests carrying the same
//! code cannot both pass.
//!
//! **Lockout and brute force.** 5 wrong codes lock the confirmation, for all
//! channels. The lock is persisted in the data directory (a restart does not
//! clear it) and backs off: 60 s, 2 min, 4 min, … capped at 1 h, the step
//! resetting on a success. A daily cap (30 wrong codes in 24 h) locks it until
//! the window ends. The lockout that starts is reported ([`ProofError::NowLocked`])
//! so the caller journals an alert. A flood of wrong codes therefore cannot
//! keep the owner out for good: **a recovery code is accepted while locked,
//! clears the lock, and never counts as a failure.**
//!
//! The record lives in the data directory, in a file of its own: the seed is
//! encrypted with the installation key, the recovery codes are stored as keyed
//! digests, and a command a trust level approved runs under a sandbox that
//! hides that directory (`core::trust_sandbox`), so the seed is out of reach of
//! the very commands the level grants.
//!
//! **The first enrolment is a trust root.** While nothing is enrolled, a code
//! proves nothing yet, so the routes that enrol (`api::trust`) ask for the
//! human-admin secret (`core::operator_secret`), which sits in the same hidden
//! directory. A pending enrolment is not replaced for [`PENDING_TTL_SECS`].
//!
//! Emergency reset (nothing in Kronn can do it for an agent): with host access
//! outside Kronn, stop Kronn and delete `trust-proof.json` (and
//! `trust-proof-lock.json`) from the data directory. Raising a level then
//! needs a new enrolment.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use serde::{Deserialize, Serialize};

use crate::core::{crypto, totp};

/// The file in the data directory.
pub const FILE: &str = "trust-proof.json";
/// The persisted failure state, next to it.
pub const LOCK_FILE: &str = "trust-proof-lock.json";
/// Wrong codes in a row before a lockout.
pub const MAX_FAILURES: u32 = 5;
/// Seconds of the first lockout; each next one doubles, up to [`MAX_LOCKOUT_SECS`].
pub const LOCKOUT_SECS: u64 = 60;
/// The longest a lockout lasts.
pub const MAX_LOCKOUT_SECS: u64 = 3600;
/// Wrong codes allowed in one 24 h window; past it the confirmation stays
/// locked until the window ends.
pub const DAILY_FAILURE_CAP: u32 = 30;
/// Single-use recovery codes issued at enrolment.
pub const RECOVERY_CODES: usize = 10;
/// How long a seed that was issued but not confirmed protects its enrolment
/// from being replaced.
pub const PENDING_TTL_SECS: u64 = 15 * 60;

const DAY_SECS: u64 = 86_400;
/// Characters of a recovery code, separators aside.
const RECOVERY_LEN: usize = 12;

/// The way a code reached Kronn. Journalled with every use and refusal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The Kronn screen (a browser request).
    Ui,
    /// Plain HTTP: `X-Kronn-Confirm` or a body field, from a script or a CLI.
    #[default]
    Http,
    /// A bridge-token request: an agent the owner gave the code to.
    Bridge,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Http => "http",
            Self::Bridge => "bridge",
        }
    }
}

/// What a code is spent on: the route and the object of the one call that
/// presented it. A code consumed here is refused everywhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub channel: Channel,
    /// `METHOD /pattern`, e.g. `PUT /api/trust/rooms/{id}`.
    pub route: String,
    /// The object of the call: `instance`, `room:<id>`, `workflow:<id>`, `proof`.
    pub object: String,
}

const ISSUER: &str = "Kronn";
const RECOVERY_DOMAIN: &[u8] = b"kronn.trust-proof.recovery.v1";
const CODE_ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    version: u32,
    /// AES-256-GCM of the base32 seed, base64.
    seed: String,
    /// The first code was confirmed: the record protects raises.
    confirmed: bool,
    enrolled_at: String,
    /// When the seed was issued (seconds); a pending enrolment is not replaced
    /// for [`PENDING_TTL_SECS`] after it.
    #[serde(default)]
    started: u64,
    /// The last time step accepted: a code of this step or an earlier one is a replay.
    last_step: u64,
    /// Keyed digests of the unused recovery codes.
    recovery: Vec<String>,
    /// What the last accepted code was spent on.
    #[serde(default)]
    last_use: Option<Binding>,
}

/// The failure state, persisted so a restart does not clear a lockout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct LockState {
    /// Wrong codes since the last lockout or success.
    failures: u32,
    /// Lockouts since the last success: the backoff step.
    lockouts: u32,
    /// Unix seconds until which the confirmation is locked.
    locked_until: u64,
    /// Start of the current 24 h window.
    day_start: u64,
    /// Wrong codes in the current window.
    day_failures: u32,
}

/// What the screen shows about the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofStatus {
    /// A confirmed enrolment exists: raising a level can be proven.
    pub enrolled: bool,
    /// A seed was issued but its first code was not confirmed yet.
    pub pending: bool,
    pub recovery_codes_left: usize,
    /// Seconds left of a lockout; 0 when not locked.
    pub locked_for_secs: u64,
}

/// Why a proof or an enrolment step was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofError {
    /// No confirmed enrolment: raising above Standard is refused.
    NotEnrolled,
    /// No proof was supplied, or it does not match (or it is a replay).
    Invalid,
    /// Too many failures: try again in this many seconds (a recovery code unlocks).
    Locked(u64),
    /// This wrong code was the one that started a lockout of this many seconds.
    NowLocked(u64),
    /// A wrong recovery-shaped code was presented while locked: refused, counted
    /// toward the daily cap, and journalled (the seconds left of the lockout).
    RecoveryRefused(u64),
    /// A seed was issued less than [`PENDING_TTL_SECS`] ago and not confirmed yet.
    PendingEnrolment,
    /// The record could not be read or written.
    Storage(String),
}

impl std::fmt::Display for ProofError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnrolled => f.write_str(
                "no strong confirmation is enrolled: enrol an authenticator in Settings › Trust levels first",
            ),
            Self::Invalid => f.write_str(
                "the confirmation code is missing, wrong or already used: enter the current code of your authenticator, or a recovery code",
            ),
            Self::Locked(secs) | Self::RecoveryRefused(secs) => write!(
                f,
                "too many wrong codes: the confirmation is locked for {secs} more seconds (a recovery code unlocks it)"
            ),
            Self::NowLocked(secs) => write!(
                f,
                "the confirmation code is wrong, and after repeated failures the confirmation is locked for {secs} seconds (a recovery code unlocks it)"
            ),
            Self::PendingEnrolment => f.write_str(
                "an enrolment is already pending: finish it with the first code of the authenticator, or wait a few minutes before starting another",
            ),
            Self::Storage(error) => write!(f, "the confirmation record is unavailable: {error}"),
        }
    }
}

/// The seed to show once, to enrol an authenticator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrolment {
    pub secret: String,
    pub uri: String,
}

/// The one critical section of every read-modify-write of the record and the
/// lock state. Held across verify, consume and write.
fn exclusive() -> MutexGuard<'static, ()> {
    static PROOF_LOCK: Mutex<()> = Mutex::new(());
    PROOF_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join(LOCK_FILE)
}

fn storage(error: impl std::fmt::Display) -> ProofError {
    ProofError::Storage(error.to_string())
}

fn read(dir: &Path) -> Result<Option<Record>, ProofError> {
    match std::fs::read_to_string(path(dir)) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(storage),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(storage(error)),
    }
}

/// Written whole or not at all (a private temporary file of its own, renamed
/// over the target), readable by its owner only from creation.
fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), ProofError> {
    use std::io::Write;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir).map_err(storage)?;
    let temporary = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join(name))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(storage(error));
    }
    Ok(())
}

fn write(dir: &Path, record: &Record) -> Result<(), ProofError> {
    let text = serde_json::to_string_pretty(record).map_err(storage)?;
    write_atomic(dir, FILE, &text)
}

/// The persisted failure state; a missing or unreadable file is a clean state
/// (it sits in the hidden data directory, nothing a command can corrupt).
fn read_lock(dir: &Path) -> LockState {
    std::fs::read_to_string(lock_path(dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_lock(dir: &Path, state: &LockState) -> Result<(), ProofError> {
    if *state == LockState::default() {
        return match std::fs::remove_file(lock_path(dir)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage(error)),
        };
    }
    write_atomic(
        dir,
        LOCK_FILE,
        &serde_json::to_string(state).map_err(storage)?,
    )
}

/// Seconds left of the lockout of `dir` at `now`.
fn locked_for(dir: &Path, now: u64) -> u64 {
    read_lock(dir).locked_until.saturating_sub(now)
}

/// A wrong code: counts toward the lockout and the daily cap. Returns the
/// length of the lockout this failure started, if it started one.
fn note_failure(dir: &Path, now: u64) -> Result<Option<u64>, ProofError> {
    Ok(count_failure(dir, now, true)?.0)
}

/// A wrong recovery-shaped code presented during a lockout: it counts toward
/// the daily cap only (the streak is already spent). Returns the failures of
/// the current 24 h window.
fn note_recovery_failure(dir: &Path, now: u64) -> Result<u32, ProofError> {
    Ok(count_failure(dir, now, false)?.1)
}

/// The lockout started by this failure, if any, and the failures of the window.
fn count_failure(dir: &Path, now: u64, in_streak: bool) -> Result<(Option<u64>, u32), ProofError> {
    let mut state = read_lock(dir);
    if now >= state.day_start + DAY_SECS {
        state.day_start = now;
        state.day_failures = 0;
    }
    state.day_failures += 1;
    let mut started = None;
    if in_streak {
        state.failures += 1;
        if state.failures >= MAX_FAILURES {
            state.failures = 0;
            state.lockouts += 1;
            let secs = LOCKOUT_SECS
                .saturating_mul(1u64 << (state.lockouts - 1).min(16))
                .min(MAX_LOCKOUT_SECS);
            state.locked_until = now + secs;
            started = Some(secs);
        }
    }
    if state.day_failures >= DAILY_FAILURE_CAP {
        let until = state.day_start + DAY_SECS;
        if until > state.locked_until {
            state.locked_until = until;
            started = Some(until.saturating_sub(now));
        }
    }
    write_lock(dir, &state)?;
    Ok((started, state.day_failures))
}

/// A code was accepted: the failure state is clean again.
fn note_success(dir: &Path) -> Result<(), ProofError> {
    write_lock(dir, &LockState::default())
}

fn seed_of(record: &Record, key: &[u8; 32]) -> Result<Vec<u8>, ProofError> {
    let text = crypto::decrypt(&record.seed, key).map_err(ProofError::Storage)?;
    totp::base32_decode(&text).ok_or_else(|| ProofError::Storage("the seed is unreadable".into()))
}

fn random_code() -> String {
    let mut bytes = [0u8; 12];
    OsRng.fill_bytes(&mut bytes);
    let letters: String = bytes
        .iter()
        .map(|b| CODE_ALPHABET[(*b & 31) as usize] as char)
        .collect();
    format!("{}-{}-{}", &letters[..4], &letters[4..8], &letters[8..])
}

fn normalized(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn recovery_digest(key: &[u8; 32], code: &str) -> String {
    crypto::keyed_digest(key, RECOVERY_DOMAIN, normalized(code).as_bytes())
}

/// Ten fresh codes: the digests to store and the plain codes to show once.
fn fresh_recovery(key: &[u8; 32]) -> (Vec<String>, Vec<String>) {
    let plain: Vec<String> = (0..RECOVERY_CODES).map(|_| random_code()).collect();
    let digests = plain.iter().map(|c| recovery_digest(key, c)).collect();
    (digests, plain)
}

/// The confirmation's state at `now`.
pub fn status(dir: &Path, now: u64) -> Result<ProofStatus, ProofError> {
    let record = read(dir)?;
    Ok(ProofStatus {
        enrolled: record.as_ref().is_some_and(|r| r.confirmed),
        pending: record.as_ref().is_some_and(|r| !r.confirmed),
        recovery_codes_left: record
            .as_ref()
            .filter(|r| r.confirmed)
            .map_or(0, |r| r.recovery.len()),
        locked_for_secs: locked_for(dir, now),
    })
}

/// Whether a confirmed enrolment exists.
pub fn is_enrolled(dir: &Path) -> Result<bool, ProofError> {
    Ok(read(dir)?.is_some_and(|r| r.confirmed))
}

/// Issue a seed. The record stays unconfirmed until [`confirm_enrolment`]
/// accepts a first code.
///
/// A confirmed enrolment is replaced only with `replace_confirmed`, which the
/// caller sets after proving the current one ([`check`]). A pending enrolment
/// (a seed issued and not confirmed yet) is never replaced during
/// [`PENDING_TTL_SECS`]: whoever started it keeps it.
pub fn begin_enrolment(
    dir: &Path,
    key: &[u8; 32],
    account: &str,
    now: u64,
    replace_confirmed: bool,
) -> Result<Enrolment, ProofError> {
    let _guard = exclusive();
    match read(dir)? {
        Some(record) if record.confirmed && !replace_confirmed => return Err(ProofError::Invalid),
        Some(record) if !record.confirmed && now < record.started + PENDING_TTL_SECS => {
            return Err(ProofError::PendingEnrolment);
        }
        _ => {}
    }
    let mut seed = [0u8; totp::SEED_BYTES];
    OsRng.fill_bytes(&mut seed);
    let secret = totp::base32_encode(&seed);
    let record = Record {
        version: 1,
        seed: crypto::encrypt(&secret, key).map_err(ProofError::Storage)?,
        confirmed: false,
        enrolled_at: chrono::Utc::now().to_rfc3339(),
        started: now,
        last_step: 0,
        recovery: Vec::new(),
        last_use: None,
    };
    write(dir, &record)?;
    Ok(Enrolment {
        uri: totp::provisioning_uri(ISSUER, account, &seed),
        secret,
    })
}

/// Confirm the first code of a pending enrolment. Returns the ten recovery
/// codes, shown once: only their digests are kept.
pub fn confirm_enrolment(
    dir: &Path,
    key: &[u8; 32],
    code: &str,
    now: u64,
) -> Result<Vec<String>, ProofError> {
    let _guard = exclusive();
    let locked = locked_for(dir, now);
    if locked > 0 {
        return Err(ProofError::Locked(locked));
    }
    let Some(mut record) = read(dir)? else {
        return Err(ProofError::NotEnrolled);
    };
    if record.confirmed {
        return Err(ProofError::Invalid);
    }
    let seed = seed_of(&record, key)?;
    let Some(step) = totp::verify(&seed, code, now) else {
        return Err(match note_failure(dir, now)? {
            Some(secs) => ProofError::NowLocked(secs),
            None => ProofError::Invalid,
        });
    };
    note_success(dir)?;
    let (digests, plain) = fresh_recovery(key);
    record.confirmed = true;
    record.last_step = step;
    record.recovery = digests;
    write(dir, &record)?;
    Ok(plain)
}

/// Check a proof: the current TOTP code, or an unused recovery code (which
/// is spent). A code of a step already accepted is a replay and is refused.
pub fn check(dir: &Path, key: &[u8; 32], proof: &str, now: u64) -> Result<(), ProofError> {
    check_inner(dir, key, proof, now, None)
}

/// [`check`] for a call: the accepted code is spent on `binding` (its route
/// and object are recorded with it).
pub fn check_bound(
    dir: &Path,
    key: &[u8; 32],
    proof: &str,
    now: u64,
    binding: &Binding,
) -> Result<(), ProofError> {
    check_inner(dir, key, proof, now, Some(binding))
}

/// What the last accepted code was spent on.
pub fn last_use(dir: &Path) -> Result<Option<Binding>, ProofError> {
    Ok(read(dir)?.and_then(|record| record.last_use))
}

fn check_inner(
    dir: &Path,
    key: &[u8; 32],
    proof: &str,
    now: u64,
    binding: Option<&Binding>,
) -> Result<(), ProofError> {
    // Verify and consume under one lock: two requests with the same code are
    // serialised, and the second sees the step or the recovery code spent.
    let _guard = exclusive();
    let Some(mut record) = read(dir)?.filter(|r| r.confirmed) else {
        return Err(ProofError::NotEnrolled);
    };
    let locked = locked_for(dir, now);
    let digest = recovery_digest(key, proof);
    let recovery_at = (normalized(proof).len() == RECOVERY_LEN)
        .then(|| record.recovery.iter().position(|d| *d == digest))
        .flatten();
    if locked > 0 {
        // A recovery code is the way out of a lockout: it is accepted, spent,
        // and lifts it. A TOTP code is refused without being verified. A wrong
        // code of the recovery shape is a guess at a recovery code: it counts
        // toward the daily cap and is journalled (the first
        // `DAILY_FAILURE_CAP` of a window, so a flood cannot fill the journal).
        let Some(index) = recovery_at else {
            if normalized(proof).len() == RECOVERY_LEN {
                let failures = note_recovery_failure(dir, now)?;
                if failures <= DAILY_FAILURE_CAP {
                    return Err(ProofError::RecoveryRefused(locked));
                }
            }
            return Err(ProofError::Locked(locked));
        };
        record.recovery.remove(index);
        record.last_use = binding.cloned();
        write(dir, &record)?;
        note_success(dir)?;
        return Ok(());
    }
    let seed = seed_of(&record, key)?;
    if let Some(step) = totp::verify(&seed, proof, now).filter(|step| *step > record.last_step) {
        record.last_step = step;
        record.last_use = binding.cloned();
        write(dir, &record)?;
        note_success(dir)?;
        return Ok(());
    }
    if let Some(index) = recovery_at {
        record.recovery.remove(index);
        record.last_use = binding.cloned();
        write(dir, &record)?;
        note_success(dir)?;
        return Ok(());
    }
    Err(match note_failure(dir, now)? {
        Some(secs) => ProofError::NowLocked(secs),
        None => ProofError::Invalid,
    })
}

/// New recovery codes (the old ones stop working). The caller proved first.
pub fn regenerate_recovery(dir: &Path, key: &[u8; 32]) -> Result<Vec<String>, ProofError> {
    let _guard = exclusive();
    let Some(mut record) = read(dir)?.filter(|r| r.confirmed) else {
        return Err(ProofError::NotEnrolled);
    };
    let (digests, plain) = fresh_recovery(key);
    record.recovery = digests;
    write(dir, &record)?;
    Ok(plain)
}

/// Remove the enrolment. The caller proved first; raising a level needs a new
/// enrolment afterwards.
pub fn disable(dir: &Path) -> Result<(), ProofError> {
    let _guard = exclusive();
    match std::fs::remove_file(path(dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage(error)),
    }
    note_success(dir)
}

#[cfg(test)]
#[path = "trust_proof_tests.rs"]
mod trust_proof_tests;
