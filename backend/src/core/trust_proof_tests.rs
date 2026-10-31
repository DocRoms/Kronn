use super::*;

const KEY: [u8; 32] = [7u8; 32];
const NOW: u64 = 1_700_000_000;

fn seed_of_dir(dir: &Path) -> Vec<u8> {
    seed_of(&read(dir).unwrap().unwrap(), &KEY).unwrap()
}

/// An enrolled directory and its recovery codes.
fn enrolled() -> (tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    begin_enrolment(dir.path(), &KEY, "me", NOW, false).unwrap();
    let seed = seed_of_dir(dir.path());
    let codes = confirm_enrolment(
        dir.path(),
        &KEY,
        &totp::code_at(&seed, totp::step_of(NOW)),
        NOW,
    )
    .unwrap();
    (dir, codes)
}

fn code(dir: &Path, step_offset: i64) -> String {
    let step = (totp::step_of(NOW) as i64 + step_offset) as u64;
    totp::code_at(&seed_of_dir(dir), step)
}

#[test]
fn without_an_enrolment_nothing_can_be_proven() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        check(dir.path(), &KEY, "123456", NOW),
        Err(ProofError::NotEnrolled)
    );
    assert!(!status(dir.path(), NOW).unwrap().enrolled);
}

#[test]
fn an_enrolment_protects_nothing_until_its_first_code_is_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let enrolment = begin_enrolment(dir.path(), &KEY, "me", NOW, false).unwrap();
    assert!(enrolment.uri.contains(&enrolment.secret));
    let pending = status(dir.path(), NOW).unwrap();
    assert!(pending.pending && !pending.enrolled);
    let valid = code(dir.path(), 0);
    assert_eq!(
        check(dir.path(), &KEY, &valid, NOW),
        Err(ProofError::NotEnrolled)
    );
    assert_eq!(
        confirm_enrolment(dir.path(), &KEY, "000000x", NOW),
        Err(ProofError::Invalid)
    );
    let codes = confirm_enrolment(dir.path(), &KEY, &valid, NOW).unwrap();
    assert_eq!(codes.len(), RECOVERY_CODES);
    let done = status(dir.path(), NOW).unwrap();
    assert!(done.enrolled && !done.pending);
    assert_eq!(done.recovery_codes_left, RECOVERY_CODES);
}

#[test]
fn the_seed_and_the_codes_are_not_stored_in_clear() {
    let (dir, codes) = enrolled();
    let stored = std::fs::read_to_string(path(dir.path())).unwrap();
    let secret = totp::base32_encode(&seed_of_dir(dir.path()));
    assert!(!stored.contains(&secret), "the seed is stored encrypted");
    for plain in &codes {
        assert!(!stored.contains(plain), "{plain} is stored hashed");
        assert!(!stored.contains(&normalized(plain)));
    }
}

#[cfg(unix)]
#[test]
fn the_record_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, _) = enrolled();
    let mode = std::fs::metadata(path(dir.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "mode {mode:o}");
}

#[test]
fn a_code_is_accepted_once_and_refused_on_replay() {
    let (dir, _) = enrolled();
    // The enrolment spent the code of the current step: the next step is fresh.
    let next = code(dir.path(), 1);
    assert_eq!(check(dir.path(), &KEY, &next, NOW), Ok(()));
    assert_eq!(
        check(dir.path(), &KEY, &next, NOW),
        Err(ProofError::Invalid)
    );
    // A code of an earlier step than the last accepted one is a replay too.
    let earlier = code(dir.path(), 0);
    assert_eq!(
        check(dir.path(), &KEY, &earlier, NOW),
        Err(ProofError::Invalid)
    );
}

#[test]
fn the_code_used_to_confirm_the_enrolment_cannot_be_replayed_as_a_proof() {
    let (dir, _) = enrolled();
    let used = code(dir.path(), 0);
    assert_eq!(
        check(dir.path(), &KEY, &used, NOW),
        Err(ProofError::Invalid)
    );
}

#[test]
fn a_recovery_code_works_once() {
    let (dir, codes) = enrolled();
    assert_eq!(check(dir.path(), &KEY, &codes[0], NOW), Ok(()));
    assert_eq!(
        check(dir.path(), &KEY, &codes[0], NOW),
        Err(ProofError::Invalid)
    );
    // Case and dashes do not matter; the others are untouched.
    let loose = codes[1].to_uppercase().replace('-', " ");
    assert_eq!(check(dir.path(), &KEY, &loose, NOW), Ok(()));
    assert_eq!(
        status(dir.path(), NOW).unwrap().recovery_codes_left,
        RECOVERY_CODES - 2
    );
}

#[test]
fn five_failures_lock_the_confirmation_for_sixty_seconds() {
    let (dir, _) = enrolled();
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(
            check(dir.path(), &KEY, "000000", NOW),
            Err(ProofError::Invalid)
        );
    }
    // The failure that starts the lockout says so (the caller journals an alert).
    assert_eq!(
        check(dir.path(), &KEY, "000000", NOW),
        Err(ProofError::NowLocked(LOCKOUT_SECS))
    );
    // Even a valid code is refused while locked.
    let valid = code(dir.path(), 1);
    assert_eq!(
        check(dir.path(), &KEY, &valid, NOW + 10),
        Err(ProofError::Locked(LOCKOUT_SECS - 10))
    );
    assert_eq!(
        status(dir.path(), NOW + 10).unwrap().locked_for_secs,
        LOCKOUT_SECS - 10
    );
    // After the lockout a valid code is accepted again.
    let later = NOW + LOCKOUT_SECS + 1;
    let valid = totp::code_at(&seed_of_dir(dir.path()), totp::step_of(later));
    assert_eq!(check(dir.path(), &KEY, &valid, later), Ok(()));
}

#[test]
fn a_success_resets_the_failure_count() {
    let (dir, _) = enrolled();
    for _ in 0..MAX_FAILURES - 1 {
        let _ = check(dir.path(), &KEY, "000000", NOW);
    }
    let valid = code(dir.path(), 1);
    assert_eq!(check(dir.path(), &KEY, &valid, NOW), Ok(()));
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(
            check(dir.path(), &KEY, "000000", NOW),
            Err(ProofError::Invalid)
        );
    }
}

#[test]
fn the_wrong_key_cannot_read_the_seed() {
    let (dir, _) = enrolled();
    let other = [9u8; 32];
    assert!(matches!(
        check(dir.path(), &other, &code(dir.path(), 1), NOW),
        Err(ProofError::Storage(_))
    ));
}

#[test]
fn regenerating_replaces_every_recovery_code_and_disabling_removes_the_enrolment() {
    let (dir, codes) = enrolled();
    let fresh = regenerate_recovery(dir.path(), &KEY).unwrap();
    assert_eq!(fresh.len(), RECOVERY_CODES);
    assert_eq!(
        check(dir.path(), &KEY, &codes[0], NOW),
        Err(ProofError::Invalid)
    );
    assert_eq!(check(dir.path(), &KEY, &fresh[0], NOW), Ok(()));
    disable(dir.path()).unwrap();
    assert!(!status(dir.path(), NOW).unwrap().enrolled);
    assert_eq!(
        check(dir.path(), &KEY, &fresh[1], NOW),
        Err(ProofError::NotEnrolled)
    );
}

#[test]
fn a_confirmed_enrolment_is_not_replaced_without_disabling_it() {
    let (dir, _) = enrolled();
    assert_eq!(
        begin_enrolment(dir.path(), &KEY, "me", NOW, false).map(|_| ()),
        Err(ProofError::Invalid)
    );
    // Only a caller who proved the current one replaces it.
    assert!(begin_enrolment(dir.path(), &KEY, "me", NOW, true).is_ok());
}

#[test]
fn a_pending_enrolment_is_not_replaced_until_it_expires() {
    let dir = tempfile::tempdir().unwrap();
    let first = begin_enrolment(dir.path(), &KEY, "me", NOW, false).unwrap();
    for at in [NOW, NOW + 60, NOW + PENDING_TTL_SECS - 1] {
        assert_eq!(
            begin_enrolment(dir.path(), &KEY, "someone-else", at, false).map(|_| ()),
            Err(ProofError::PendingEnrolment),
            "replaced at +{}",
            at - NOW
        );
    }
    // The owner's seed is still the one on file.
    assert_eq!(
        totp::base32_encode(&seed_of_dir(dir.path())),
        first.secret,
        "the pending seed was replaced"
    );
    // Someone who lost the seed can start over once it has expired.
    let later = NOW + PENDING_TTL_SECS;
    let second = begin_enrolment(dir.path(), &KEY, "me", later, false).unwrap();
    assert_ne!(second.secret, first.secret);
}

fn binding(channel: Channel, route: &str, object: &str) -> Binding {
    Binding {
        channel,
        route: route.into(),
        object: object.into(),
    }
}

/// The code is the proof whatever the channel: it is spent on the route and
/// object of the call that presented it, recorded, and refused everywhere else.
#[test]
fn a_code_is_spent_on_one_route_and_object_and_refused_on_any_other() {
    let (dir, codes) = enrolled();
    let first = binding(Channel::Bridge, "PUT /api/trust/rooms/{id}", "room:r1");
    let valid = code(dir.path(), 1);
    assert_eq!(check_bound(dir.path(), &KEY, &valid, NOW, &first), Ok(()));
    assert_eq!(last_use(dir.path()).unwrap(), Some(first));
    for other in [
        binding(Channel::Bridge, "PUT /api/trust/rooms/{id}", "room:r2"),
        binding(Channel::Http, "PUT /api/trust/instance", "instance"),
        binding(Channel::Ui, "POST /api/trust/proof/disable", "proof"),
    ] {
        assert_eq!(
            check_bound(dir.path(), &KEY, &valid, NOW, &other),
            Err(ProofError::Invalid),
            "{other:?}"
        );
    }
    // A recovery code is spent the same way, on its one call.
    let second = binding(Channel::Http, "PUT /api/trust/instance", "instance");
    assert_eq!(
        check_bound(dir.path(), &KEY, &codes[0], NOW, &second),
        Ok(())
    );
    assert_eq!(last_use(dir.path()).unwrap(), Some(second.clone()));
    assert_eq!(
        check_bound(dir.path(), &KEY, &codes[0], NOW, &second),
        Err(ProofError::Invalid)
    );
}

/// Failures on any channel feed the one lockout.
#[test]
fn the_lockout_is_shared_by_every_channel() {
    let (dir, _) = enrolled();
    let channels = [
        Channel::Ui,
        Channel::Http,
        Channel::Bridge,
        Channel::Bridge,
        Channel::Http,
    ];
    for (index, channel) in channels.into_iter().enumerate() {
        let call = binding(channel, "PUT /api/trust/instance", "instance");
        assert_eq!(
            check_bound(dir.path(), &KEY, "000000", NOW, &call),
            Err(if index + 1 == channels.len() {
                ProofError::NowLocked(LOCKOUT_SECS)
            } else {
                ProofError::Invalid
            })
        );
    }
    let call = binding(Channel::Ui, "PUT /api/trust/instance", "instance");
    assert_eq!(
        check_bound(dir.path(), &KEY, &code(dir.path(), 1), NOW, &call),
        Err(ProofError::Locked(LOCKOUT_SECS))
    );
    assert_eq!(
        check_bound(
            dir.path(),
            &KEY,
            &code(dir.path(), 1),
            NOW + LOCKOUT_SECS,
            &call
        ),
        Ok(())
    );
}

fn fail_n(dir: &Path, times: u32, at: u64) -> Option<ProofError> {
    let mut last = None;
    for _ in 0..times {
        last = check(dir, &KEY, "000000", at).err();
    }
    last
}

fn valid_at(dir: &Path, at: u64) -> String {
    totp::code_at(&seed_of_dir(dir), totp::step_of(at))
}

/// Two requests with one code at the same instant: exactly one passes, the other
/// is a replay. Repeated, with a fresh code each round.
#[test]
fn two_simultaneous_requests_with_one_code_cannot_both_pass() {
    let (dir, _) = enrolled();
    for round in 1..=20u64 {
        let at = NOW + round * 30;
        let valid = valid_at(dir.path(), at);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["room:r", "instance"]
            .into_iter()
            .map(|object| {
                let path = dir.path().to_path_buf();
                let code = valid.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let call = binding(Channel::Http, "PUT /api/trust/x", object);
                    barrier.wait();
                    check_bound(&path, &KEY, &code, at, &call)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "round {round}: {results:?}"
        );
    }
}

#[test]
fn a_recovery_code_is_spent_once_even_when_presented_four_times_at_once() {
    let (dir, codes) = enrolled();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = dir.path().to_path_buf();
            let code = codes[0].clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                check(&path, &KEY, &code, NOW)
            })
        })
        .collect();
    let ok = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(Result::is_ok)
        .count();
    assert_eq!(ok, 1);
    assert_eq!(
        status(dir.path(), NOW).unwrap().recovery_codes_left,
        RECOVERY_CODES - 1
    );
}

/// The lockout survives a restart: it is on disk, not in memory.
#[test]
fn the_lockout_is_persisted_in_the_data_dir() {
    let (dir, _) = enrolled();
    assert!(!lock_path(dir.path()).exists());
    assert_eq!(
        fail_n(dir.path(), MAX_FAILURES, NOW),
        Some(ProofError::NowLocked(LOCKOUT_SECS))
    );
    assert!(lock_path(dir.path()).exists());
    assert_eq!(read_lock(dir.path()).locked_until, NOW + LOCKOUT_SECS);
    // What a restarted process reads.
    assert_eq!(
        status(dir.path(), NOW + 5).unwrap().locked_for_secs,
        LOCKOUT_SECS - 5
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(lock_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }
}

/// 60 s, 2 min, 4 min, … capped at 1 h; a success starts over.
#[test]
fn each_lockout_lasts_twice_as_long_up_to_an_hour() {
    let (dir, _) = enrolled();
    let mut now = NOW;
    let mut expected = vec![];
    let mut secs = LOCKOUT_SECS;
    while secs < MAX_LOCKOUT_SECS {
        expected.push(secs);
        secs *= 2;
    }
    expected.extend([MAX_LOCKOUT_SECS, MAX_LOCKOUT_SECS]);
    assert_eq!(&expected[..3], [60, 120, 240]);
    // Seven lockouts is 35 wrong codes: past the daily cap, so the lockouts are
    // spaced a day apart and each one is measured alone.
    for (lockouts_seen, want) in expected.into_iter().enumerate() {
        if lockouts_seen > 0 && lockouts_seen % 5 == 0 {
            now += DAY_SECS;
        }
        assert_eq!(
            fail_n(dir.path(), MAX_FAILURES, now),
            Some(ProofError::NowLocked(want))
        );
        assert_eq!(locked_for(dir.path(), now), want);
        now += want + 1;
        assert_eq!(locked_for(dir.path(), now), 0, "the lockout ends");
    }
    // A success resets the step.
    let valid = valid_at(dir.path(), now + 30);
    assert_eq!(check(dir.path(), &KEY, &valid, now + 30), Ok(()));
    assert_eq!(read_lock(dir.path()), LockState::default());
    assert_eq!(
        fail_n(dir.path(), MAX_FAILURES, now + 60),
        Some(ProofError::NowLocked(LOCKOUT_SECS))
    );
}

/// A flood of wrong codes cannot try more than the daily cap per window.
#[test]
fn a_daily_cap_bounds_the_guesses_whatever_the_backoff() {
    let (dir, _) = enrolled();
    let mut now = NOW;
    let mut attempts = 0;
    // Retry as soon as each lockout ends, for a whole day.
    while now < NOW + DAY_SECS {
        let locked = locked_for(dir.path(), now);
        if locked > 0 {
            now += locked;
            continue;
        }
        attempts += 1;
        let _ = check(dir.path(), &KEY, "000000", now);
        now += 1;
    }
    assert_eq!(attempts, DAILY_FAILURE_CAP, "wrong codes tried in 24 h");
    // The next window opens clean.
    let fresh = NOW + DAY_SECS + 1;
    assert_eq!(locked_for(dir.path(), fresh), 0);
    assert_eq!(
        check(dir.path(), &KEY, "000000", fresh),
        Err(ProofError::Invalid)
    );
}

/// The owner is never locked out for good: a recovery code is accepted while
/// locked, lifts the lockout, and is not counted as a failure.
#[test]
fn a_recovery_code_unlocks_and_never_counts_as_a_failure() {
    let (dir, codes) = enrolled();
    assert_eq!(
        fail_n(dir.path(), MAX_FAILURES, NOW),
        Some(ProofError::NowLocked(LOCKOUT_SECS))
    );
    // A valid TOTP code and a wrong code are refused while locked, uncounted.
    let before = read_lock(dir.path());
    assert_eq!(
        check(dir.path(), &KEY, &valid_at(dir.path(), NOW + 30), NOW + 1),
        Err(ProofError::Locked(LOCKOUT_SECS - 1))
    );
    assert_eq!(
        check(dir.path(), &KEY, "000000", NOW + 1),
        Err(ProofError::Locked(LOCKOUT_SECS - 1))
    );
    assert_eq!(read_lock(dir.path()), before, "a refused attempt counted");
    // The recovery code goes through, and the lock is gone.
    let unlock = binding(Channel::Http, "PUT /api/trust/instance", "instance");
    assert_eq!(
        check_bound(dir.path(), &KEY, &codes[0], NOW + 2, &unlock),
        Ok(())
    );
    assert_eq!(locked_for(dir.path(), NOW + 2), 0);
    assert_eq!(read_lock(dir.path()), LockState::default());
    assert_eq!(last_use(dir.path()).unwrap(), Some(unlock));
    // It is single use.
    assert_eq!(
        check(dir.path(), &KEY, &codes[0], NOW + 3),
        Err(ProofError::Invalid)
    );
}

/// A guess at a recovery code during a lockout is no free try: it is refused
/// the same way, counted toward the daily cap (not the streak), and reported as
/// journalled, up to the cap; the owner's real code still unlocks.
#[test]
fn a_wrong_recovery_code_during_a_lockout_counts_toward_the_daily_cap() {
    let (dir, codes) = enrolled();
    fail_n(dir.path(), MAX_FAILURES, NOW);
    let before = read_lock(dir.path());
    assert_eq!(
        check(dir.path(), &KEY, "abcd-efgh-jkmn", NOW + 1),
        Err(ProofError::RecoveryRefused(LOCKOUT_SECS - 1))
    );
    let after = read_lock(dir.path());
    assert_eq!(after.day_failures, before.day_failures + 1);
    assert_eq!(after.failures, before.failures, "the streak is not touched");
    assert_eq!(after.lockouts, before.lockouts, "no extra backoff step");
    // The cap holds for these guesses too: past it the window stays locked and
    // the refusal is no longer journalled.
    let mut last = None;
    for i in 0..DAILY_FAILURE_CAP {
        last = check(dir.path(), &KEY, "abcd-efgh-jkmn", NOW + 2 + u64::from(i)).err();
    }
    assert!(matches!(last, Some(ProofError::Locked(_))), "{last:?}");
    assert_eq!(
        read_lock(dir.path()).locked_until,
        read_lock(dir.path()).day_start + DAY_SECS
    );
    // The owner's recovery code still unlocks.
    assert_eq!(check(dir.path(), &KEY, &codes[0], NOW + 100), Ok(()));
    assert_eq!(read_lock(dir.path()), LockState::default());
}

/// Enrolment confirmation shares the same lockout.
#[test]
fn wrong_first_codes_lock_the_enrolment_too() {
    let dir = tempfile::tempdir().unwrap();
    begin_enrolment(dir.path(), &KEY, "me", NOW, false).unwrap();
    for _ in 0..MAX_FAILURES - 1 {
        assert_eq!(
            confirm_enrolment(dir.path(), &KEY, "000000", NOW).map(|_| ()),
            Err(ProofError::Invalid)
        );
    }
    assert_eq!(
        confirm_enrolment(dir.path(), &KEY, "000000", NOW).map(|_| ()),
        Err(ProofError::NowLocked(LOCKOUT_SECS))
    );
    let valid = valid_at(dir.path(), NOW);
    assert_eq!(
        confirm_enrolment(dir.path(), &KEY, &valid, NOW + 1).map(|_| ()),
        Err(ProofError::Locked(LOCKOUT_SECS - 1))
    );
}
