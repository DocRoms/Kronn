use super::*;

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO discussions (id,title,agent,created_at,updated_at) \
         VALUES ('d-room','D','Codex','now','now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id,title,agent,created_at,updated_at) \
         VALUES ('d-other','O','Codex','now','now')",
        [],
    )
    .unwrap();
    conn
}

/// Bootstrap + one enrolled credential: the ordinary starting point.
fn enrolled(conn: &Connection) -> (Secret, Secret) {
    let admin = create_admin_secret(conn, "/operator/private/kronn-admin-secret").unwrap();
    let authority = authorise_enrolment(conn, &admin).unwrap().unwrap();
    let (_credential, human) = enrol(conn, &authority, "Romu — laptop").unwrap();
    (admin, human)
}

const BODY: &str = "```kronn-important\n{\"version\":1}\n```";

// ── Bootstrap ───────────────────────────────────────────────────────────────

#[test]
fn the_admin_secret_is_minted_once_and_only_its_hash_is_kept() {
    let conn = database();
    let admin = create_admin_secret(&conn, "/operator/private/secret").unwrap();

    let stored: String = conn
        .query_row("SELECT secret_hash FROM human_admin_secret", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(stored, admin.expose(), "the plaintext must not be stored");
    assert_eq!(stored, sha256_hex(admin.expose()));

    // The row records WHERE the operator can find it, never the secret.
    assert_eq!(
        admin_secret_location(&conn).unwrap().as_deref(),
        Some("/operator/private/secret")
    );

    // A second mint would silently authorise enrolments the operator never
    // sanctioned, so it is refused rather than layered on.
    assert!(create_admin_secret(&conn, "/elsewhere").is_err());
}

#[test]
fn without_a_bootstrap_nothing_can_enrol() {
    let conn = database();
    // The "no TOFU" property: an install with no admin secret and no
    // credential authorises nobody, including whoever asks first.
    for guess in ["kr-admin-anything", "", "kr-human-anything"] {
        assert!(authorise_enrolment(&conn, &Secret::new(guess))
            .unwrap()
            .is_none());
    }
}

#[test]
fn rotating_the_admin_secret_keeps_the_credentials_it_enrolled() {
    let conn = database();
    let (admin, human) = enrolled(&conn);

    let rotated = rotate_admin_secret(&conn, "/operator/private/secret-2").unwrap();
    assert_ne!(rotated.expose(), admin.expose());

    // The old bootstrap stops enrolling…
    assert!(authorise_enrolment(&conn, &admin).unwrap().is_none());
    // …the new one starts…
    assert!(authorise_enrolment(&conn, &rotated).unwrap().is_some());
    // …and a credential already enrolled is untouched: rotation of the
    // bootstrap is not a revocation of everything it ever signed.
    assert!(authenticate_human(&conn, &human).is_ok());
}

// ── Enrolment ───────────────────────────────────────────────────────────────

#[test]
fn a_live_human_credential_can_enrol_another() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);

    let authority = authorise_enrolment(&conn, &human).unwrap().unwrap();
    let (second, secret) = enrol(&conn, &authority, "Romu — phone").unwrap();
    assert_eq!(second.enrolled_by, EnrolledBy::Human);
    assert!(authenticate_human(&conn, &secret).is_ok());

    // Recovery without the admin secret is exactly this path, which is why it
    // needs no anonymous reset.
    assert_eq!(list(&conn).unwrap().len(), 2);
}

#[test]
fn a_revoked_credential_can_no_longer_enrol_or_publish() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let id = authenticate_human(&conn, &human).unwrap().0;

    assert!(revoke(&conn, &id, "laptop lost").unwrap());
    assert_eq!(
        authenticate_human(&conn, &human),
        Err(AuthorityError::Revoked)
    );
    assert!(authorise_enrolment(&conn, &human).unwrap().is_none());
    // Revoking twice is not an error the caller should act on, but it is not a
    // second revocation either.
    assert!(!revoke(&conn, &id, "again").unwrap());
}

#[test]
fn a_label_is_required_and_bounded() {
    let conn = database();
    let admin = create_admin_secret(&conn, "/p").unwrap();
    let authority = authorise_enrolment(&conn, &admin).unwrap().unwrap();
    assert!(enrol(&conn, &authority, "   ").is_err());
    assert!(enrol(&conn, &authority, &"x".repeat(101)).is_err());
    assert!(enrol(&conn, &authority, &"é".repeat(100)).is_ok());
}

// ── Proofs ──────────────────────────────────────────────────────────────────

#[test]
fn a_proof_is_spent_once_and_binds_room_and_content() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let now = Utc::now();

    let proof = issue_proof(&conn, &human, "d-room", BODY, now).unwrap();
    assert!(consume_proof(&conn, &proof, &human, "d-room", BODY, now).is_ok());

    // Spent once, and no amount of asking makes it spendable again.
    assert_eq!(
        consume_proof(&conn, &proof, &human, "d-room", BODY, now),
        Err(ProofError::AlreadyUsed)
    );
}

#[test]
fn a_proof_cannot_be_moved_to_another_room_or_another_body() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let now = Utc::now();

    let proof = issue_proof(&conn, &human, "d-room", BODY, now).unwrap();
    assert_eq!(
        consume_proof(&conn, &proof, &human, "d-other", BODY, now),
        Err(ProofError::WrongDiscussion)
    );

    let tampered = BODY.replace("\"version\":1", "\"version\":1 ");
    assert_eq!(
        consume_proof(&conn, &proof, &human, "d-room", &tampered, now),
        Err(ProofError::ContentChanged),
        "one changed byte is a different card"
    );

    // Neither refusal spent it: the legitimate publication still works.
    assert!(consume_proof(&conn, &proof, &human, "d-room", BODY, now).is_ok());
}

#[test]
fn a_proof_expires() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let now = Utc::now();
    let proof = issue_proof(&conn, &human, "d-room", BODY, now).unwrap();

    let late = now + Duration::seconds(PROOF_TTL_SECONDS + 1);
    assert_eq!(
        consume_proof(&conn, &proof, &human, "d-room", BODY, late),
        Err(ProofError::Expired)
    );
    // On the boundary it is still good; expiry is a deadline, not a race.
    let just_in_time = now + Duration::seconds(PROOF_TTL_SECONDS);
    assert!(consume_proof(&conn, &proof, &human, "d-room", BODY, just_in_time).is_ok());
}

#[test]
fn a_broken_expiry_refuses_rather_than_opening_the_door() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let now = Utc::now();
    let proof = issue_proof(&conn, &human, "d-room", BODY, now).unwrap();
    conn.execute(
        "UPDATE human_publication_proofs SET expires_at = 'not-a-date' WHERE id = ?1",
        params![proof],
    )
    .unwrap();
    assert_eq!(
        consume_proof(&conn, &proof, &human, "d-room", BODY, now),
        Err(ProofError::Expired)
    );
}

#[test]
fn revocation_and_rotation_kill_proofs_already_in_flight() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let now = Utc::now();
    let id = authenticate_human(&conn, &human).unwrap().0;

    let issued_before = issue_proof(&conn, &human, "d-room", BODY, now).unwrap();
    let rotated = rotate_human(&conn, &id).unwrap();
    // The old secret is dead, so the proof it holds is unreachable…
    assert_eq!(
        consume_proof(&conn, &issued_before, &human, "d-room", BODY, now),
        Err(ProofError::Unknown)
    );
    // …and the new secret cannot spend a proof issued under the old epoch.
    assert_eq!(
        consume_proof(&conn, &issued_before, &rotated, "d-room", BODY, now),
        Err(ProofError::CredentialChanged),
        "rotation must invalidate what was already in flight"
    );

    let issued_after = issue_proof(&conn, &rotated, "d-room", BODY, now).unwrap();
    assert!(revoke(&conn, &id, "revoked mid-flight").unwrap());
    assert_eq!(
        consume_proof(&conn, &issued_after, &rotated, "d-room", BODY, now),
        Err(ProofError::Unknown)
    );
}

#[test]
fn a_proof_belonging_to_someone_else_is_simply_unknown() {
    let conn = database();
    let (admin, first) = enrolled(&conn);
    let authority = authorise_enrolment(&conn, &admin).unwrap().unwrap();
    let (_second, other) = enrol(&conn, &authority, "someone else").unwrap();
    let now = Utc::now();

    let proof = issue_proof(&conn, &first, "d-room", BODY, now).unwrap();
    // Not `WrongDiscussion`, not `AlreadyUsed`: the caller learns nothing about
    // a credential that is not its own.
    assert_eq!(
        consume_proof(&conn, &proof, &other, "d-room", BODY, now),
        Err(ProofError::Unknown)
    );
}

#[test]
fn an_unknown_credential_gets_no_proof_at_all() {
    let conn = database();
    enrolled(&conn);
    let now = Utc::now();
    for stranger in ["kr-human-invented", ""] {
        assert_eq!(
            issue_proof(&conn, &Secret::new(stranger), "d-room", BODY, now),
            Err(AuthorityError::Unknown)
        );
    }
}

// ── Non-exposure ────────────────────────────────────────────────────────────

#[test]
fn a_secret_never_prints_itself() {
    let secret = Secret::new("kr-human-must-never-appear");
    let printed = format!("{secret:?}");
    assert!(!printed.contains("must-never-appear"), "leaked: {printed}");
    assert!(printed.contains("<redacted>"));
}

#[test]
fn the_credential_listing_carries_no_secret_material() {
    let conn = database();
    let (_admin, human) = enrolled(&conn);
    let listed = list(&conn).unwrap();
    let rendered = serde_json::to_string(&listed).unwrap();

    assert!(
        !rendered.contains(human.expose()),
        "plaintext in the listing"
    );
    assert!(
        !rendered.contains(&sha256_hex(human.expose())),
        "even the hash must not travel: it is enough to verify a guess"
    );
    assert!(rendered.contains("Romu — laptop"));
}
