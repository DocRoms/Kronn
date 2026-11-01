//! KT-1492 — the trust policy: which level grants which approval on its own.
//!
//! One table serves every gate and the risk panel, so what the panel lists is
//! what the gates do. An approval that is not in the table is never granted by
//! a level (see [`NeverCovered`]).

use crate::models::{NeverCovered, TrustApproval, TrustEffect, TrustLevel};

/// The Settings section that holds the global trust level (`Settings › Sécurité`).
pub const SETTINGS_SECTION: &str = "settings-trust";

/// The lowest level that grants `approval`.
pub fn minimum_level(approval: TrustApproval) -> TrustLevel {
    match approval {
        TrustApproval::QuickExecLine
        | TrustApproval::ExecStepLine
        | TrustApproval::RoomScopeRead
        | TrustApproval::RoomScopePlan
        | TrustApproval::ValidationsDocker => TrustLevel::Trusted,
        TrustApproval::CodeWriteOtherProject
        | TrustApproval::ValidationsUnconfined
        | TrustApproval::HostAccess => TrustLevel::Full,
    }
}

/// Whether a gate in this build asks the table for `approval`. Code write on
/// other projects (KT-1491) and host access for agents have their gate in
/// another ticket: they are in the policy so that gate has one place to ask,
/// and the panel says they are not enforced yet.
pub fn is_enforced(approval: TrustApproval) -> bool {
    !matches!(
        approval,
        TrustApproval::CodeWriteOtherProject | TrustApproval::HostAccess
    )
}

/// Whether `level` grants `approval` without asking.
pub fn grants(level: TrustLevel, approval: TrustApproval) -> bool {
    level >= minimum_level(approval)
}

/// Whether any level covers `kind`. Always `false`: these are held by their own
/// gates (credential stripping, human-only routes, delete confirmations,
/// federation), none of which reads a trust level.
pub fn covers(_level: TrustLevel, _kind: NeverCovered) -> bool {
    false
}

/// The policy table, in the order the risk panel lists it.
pub fn effects() -> Vec<TrustEffect> {
    TrustApproval::ALL
        .into_iter()
        .map(|approval| TrustEffect {
            approval,
            from_level: minimum_level(approval),
            enforced: is_enforced(approval),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered_and_standard_grants_nothing() {
        assert!(TrustLevel::Standard < TrustLevel::Trusted);
        assert!(TrustLevel::Trusted < TrustLevel::Full);
        for approval in TrustApproval::ALL {
            assert!(
                !grants(TrustLevel::Standard, approval),
                "{approval:?} must ask under Standard"
            );
        }
    }

    #[test]
    fn trusted_grants_the_ticket_examples_and_not_the_full_ones() {
        use TrustApproval::*;
        for approval in [
            QuickExecLine,
            ExecStepLine,
            RoomScopeRead,
            RoomScopePlan,
            ValidationsDocker,
        ] {
            assert!(grants(TrustLevel::Trusted, approval), "{approval:?}");
        }
        for approval in [CodeWriteOtherProject, ValidationsUnconfined, HostAccess] {
            assert!(
                !grants(TrustLevel::Trusted, approval),
                "{approval:?} is Full access only"
            );
        }
    }

    #[test]
    fn full_access_grants_everything_trusted_does_plus_its_own() {
        for approval in TrustApproval::ALL {
            assert!(grants(TrustLevel::Full, approval), "{approval:?}");
        }
    }

    #[test]
    fn code_write_is_asked_under_standard_and_trusted_and_granted_under_full() {
        // KT-1491 builds on this row.
        let code = TrustApproval::CodeWriteOtherProject;
        assert!(!grants(TrustLevel::Standard, code));
        assert!(!grants(TrustLevel::Trusted, code));
        assert!(grants(TrustLevel::Full, code));
    }

    #[test]
    fn no_level_covers_what_is_never_covered() {
        for level in [TrustLevel::Standard, TrustLevel::Trusted, TrustLevel::Full] {
            for kind in NeverCovered::ALL {
                assert!(!covers(level, kind), "{kind:?} under {level:?}");
            }
        }
    }

    #[test]
    fn the_never_covered_kinds_are_not_approvals_of_the_table() {
        // A secret, a trust change, a delete confirmation or a peer's trust
        // cannot be expressed as a grantable approval.
        for approval in TrustApproval::ALL {
            let name = approval.as_str();
            for forbidden in ["credential", "secret", "token", "trust", "delete", "peer"] {
                assert!(!name.contains(forbidden), "{name} names {forbidden}");
            }
        }
    }

    #[test]
    fn the_panel_table_lists_every_approval_once_with_its_minimum_level() {
        let table = effects();
        assert_eq!(table.len(), TrustApproval::ALL.len());
        for effect in table {
            assert_eq!(effect.from_level, minimum_level(effect.approval));
            assert_ne!(effect.from_level, TrustLevel::Standard);
        }
    }

    #[test]
    fn the_approvals_with_no_gate_yet_are_flagged_not_enforced() {
        let not_enforced: Vec<_> = effects()
            .into_iter()
            .filter(|effect| !effect.enforced)
            .map(|effect| effect.approval)
            .collect();
        assert_eq!(
            not_enforced,
            vec![
                TrustApproval::CodeWriteOtherProject,
                TrustApproval::HostAccess
            ]
        );
    }

    #[test]
    fn approvals_round_trip_through_their_stored_name() {
        for approval in TrustApproval::ALL {
            assert_eq!(TrustApproval::parse(approval.as_str()), Some(approval));
        }
        assert_eq!(TrustApproval::parse("nope"), None);
    }

    /// DoD 3: the gates that hold the exclusions are not parameterised by a
    /// trust level and never read one, so no level can reach them.
    #[test]
    fn the_gates_that_hold_the_exclusions_never_read_a_trust_level() {
        let held_by_their_own_gate = [
            (
                "bridge_token (credential strip, human-only routes)",
                include_str!("bridge_token.rs"),
            ),
            ("secret_scrub", include_str!("secret_scrub.rs")),
            ("redact", include_str!("redact.rs")),
            ("secret_facts", include_str!("secret_facts.rs")),
            ("credential_store", include_str!("credential_store.rs")),
            ("export_secrets", include_str!("export_secrets.rs")),
            ("mcp_secret_refs", include_str!("mcp_secret_refs.rs")),
            (
                "human_credentials",
                include_str!("../api/human_credentials.rs"),
            ),
            (
                "federation (a P2P peer)",
                include_str!("../api/federation.rs"),
            ),
            (
                "disc_portability",
                include_str!("../api/disc_portability.rs"),
            ),
        ];
        for (name, source) in held_by_their_own_gate {
            for needle in [
                "db::trust",
                "core::trust",
                "TrustLevel",
                "TrustOrigin",
                "TrustApproval",
            ] {
                assert!(
                    !source.contains(needle),
                    "{name} reads a trust level ({needle})"
                );
            }
        }
    }

    /// DoD 3: the trust storage touches no secret, and its routes are not
    /// bridge routes.
    #[test]
    fn the_trust_module_exposes_no_secret_and_no_bridge_route() {
        let api = include_str!("../api/trust.rs");
        assert!(!api.contains("BRIDGE_ROUTES"));
        let storage = include_str!("../db/trust.rs");
        for needle in ["token_override", "credential", "api_key"] {
            assert!(!storage.contains(needle), "db::trust touches {needle}");
        }
    }

    /// The one-time-password code is the same whatever the level: it never reads one.
    #[test]
    fn the_strong_confirmation_never_reads_a_trust_level() {
        for (name, source) in [
            ("totp", include_str!("totp.rs")),
            ("trust_proof", include_str!("trust_proof.rs")),
        ] {
            for needle in ["db::trust", "TrustLevel", "TrustOrigin", "TrustApproval"] {
                assert!(!source.contains(needle), "{name} reads a trust level");
            }
        }
    }

    /// Autoreview 🔴 1: a trust setting is raised by the human routes alone, and
    /// those need the strong confirmation; no other module of the backend sets
    /// or clears a level (tests excepted).
    #[test]
    fn only_the_human_routes_set_or_clear_a_level() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy();
            let is_test = name.ends_with("_tests.rs")
                || name.ends_with("_test.rs")
                || name == "tests.rs"
                || path.components().any(|part| part.as_os_str() == "tests");
            let is_owner = path.ends_with("api/trust.rs")
                || path.ends_with("db/trust.rs")
                || path.ends_with("core/trust.rs");
            if is_test || is_owner {
                continue;
            }
            let text = std::fs::read_to_string(path).unwrap();
            for needle in ["trust::set_level(", "trust::clear_level("] {
                if text.contains(needle) {
                    offenders.push(format!("{} calls {needle}", path.display()));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:?}");
        // And the human route asks for the proof before it stores a raise.
        let api = include_str!("../api/trust.rs");
        let proof = api.find("require_proof::<TrustScopeView>").unwrap();
        let store = api.find("trust::set_level(conn").unwrap();
        assert!(proof < store, "set_level must follow the proof check");
    }

    /// Autoreview 🔴 1 / 🟡 4: every secret Kronn keeps lives in the directory the
    /// sandbox hides from what a level approves. Moving one out of it, or
    /// dropping the data directory from the hidden set, fails here.
    #[test]
    #[serial_test::serial]
    fn every_secret_kronn_keeps_is_inside_what_the_sandbox_hides() {
        let backups = tempfile::tempdir().unwrap();
        std::env::set_var("KRONN_BACKUP_DIR", backups.path());
        let hidden = crate::core::trust_sandbox::protected_dirs();
        std::env::remove_var("KRONN_BACKUP_DIR");
        let data = crate::core::config::config_dir().unwrap();
        let covered = |path: &std::path::Path| hidden.iter().any(|dir| path.starts_with(dir));
        for name in [
            crate::core::keyvault::SIDECAR_FILENAME,
            crate::core::recovery::RECOVERY_FILENAME,
            "config.toml",
            "kronn.db",
            "kronn.db-wal",
            "kronn.db.backup",
            "backups/kronn.db.bak",
            crate::core::trust_proof::FILE,
            crate::core::trust_proof::LOCK_FILE,
        ] {
            assert!(covered(&data.join(name)), "{name} is reachable");
        }
        assert!(
            covered(&crate::core::operator_secret::secret_path().unwrap()),
            "human-admin-secret is reachable"
        );
        assert!(
            covered(&backups.path().join("kronn.db.bak")),
            "a backup directory kept elsewhere is reachable"
        );
    }
}
