use super::*;
use crate::core::keyvault::{KeyVault, SidecarFile};
use crate::db::migrations::{backup_config_sealed, rotate_config_backup};

/// A data directory whose key file holds a key, as an installed Kronn's does.
fn dir_with_key() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let key = crypto::generate_secret();
    SidecarFile::in_dir(dir.path()).store(&key).unwrap();
    (dir, key)
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// The files of `dir` (hidden temp files included, `skip` aside) whose bytes
/// hold one of `needles`.
fn files_holding(dir: &Path, needles: &[&str], skip: &[&str]) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| !skip.contains(&n.as_str()))
        .filter(|n| {
            let bytes = std::fs::read(dir.join(n)).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            needles.iter().any(|needle| text.contains(needle))
        })
        .collect()
}

/// Credentials in the shapes a list of names misses: unusual names, values in
/// arrays, URLs, name/value tables.
const LEAKY: &str = "language = \"fr\"\n\
    pat = \"pat-clear\"\n\
    github_pat = \"ghpat-clear\"\n\
    cookie = \"cookie-clear\"\n\
    accesstoken = \"acc-clear\"\n\
    privatekey = \"pk-clear\"\n\
    OpenAIKey = \"oai-clear\"\n\
    database_url = \"postgres://user:dbpass-clear@db/app\"\n\
    webhook_url = \"https://hooks.example/T000/B000/hook-clear\"\n\n\
    [future]\n\
    args = [\"--api-key\", \"argkey-clear\"]\n\
    header = \"Authorization: Bearer bearer-clear\"\n\
    note = \"token ghp_tokenclear\"\n\
    endpoint = \"https://api.example/v1?api_key=query-clear\"\n\
    headers = [{ name = \"X-Api-Key\", value = \"nv-clear\" }]\n\n\
    [tokens]\nmistral = \"sk-mistral-clear\"\n";

/// A broken section whose multi-line string carries a credential line.
const BROKEN: &str =
    "language = \"fr\"\n[server\nnote = \"\"\"\napi_key = \"multi-clear\"\n\"\"\"\n\
    [[tokens.keys]]\nvalue = \"sk-broken-clear\"\n";

const NEEDLES: &[&str] = &[
    "pat-clear",
    "ghpat-clear",
    "cookie-clear",
    "acc-clear",
    "pk-clear",
    "oai-clear",
    "dbpass-clear",
    "hook-clear",
    "argkey-clear",
    "bearer-clear",
    "ghp_tokenclear",
    "query-clear",
    "nv-clear",
    "sk-mistral-clear",
    "multi-clear",
    "sk-broken-clear",
];

/// KT-1528 — the migration snapshot is encrypted whole: each credential shape
/// a name list would miss leaves no clear value in any file of the directory
/// (the temp files included), and the copy still restores the file exactly.
#[test]
fn the_migration_snapshot_leaves_no_clear_value_for_any_credential_shape() {
    for text in [LEAKY, BROKEN] {
        let (dir, key) = dir_with_key();
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        backup_config_sealed(dir.path()).unwrap();
        // Run again, as every start with a pending migration does.
        backup_config_sealed(dir.path()).unwrap();

        let leaks = files_holding(dir.path(), NEEDLES, &["config.toml"]);
        assert!(leaks.is_empty(), "clear values in {leaks:?}");
        let backup = dir.path().join("config.toml.backup.enc");
        assert_eq!(open_sealed(&backup, &[key]).unwrap(), text);
        assert!(!dir.path().join("config.toml.backup").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Idempotent: the same file is not copied again.
        assert_eq!(copies(dir.path()).len(), 1);
    }
}

/// An older snapshot with other content is kept (a rotation), sealed too.
#[test]
fn a_changed_config_rotates_the_older_snapshot_and_both_are_sealed() {
    let (dir, key) = dir_with_key();
    std::fs::write(dir.path().join("config.toml"), LEAKY).unwrap();
    backup_config_sealed(dir.path()).unwrap();
    let changed = LEAKY.replace("language = \"fr\"", "language = \"en\"");
    std::fs::write(dir.path().join("config.toml"), &changed).unwrap();
    backup_config_sealed(dir.path()).unwrap();

    let all = copies(dir.path());
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all.iter().all(|c| c.sealed && c.kind == Kind::Backup));
    let texts: Vec<String> = all
        .iter()
        .map(|c| open_sealed(&c.path, std::slice::from_ref(&key)).unwrap())
        .collect();
    assert!(texts.contains(&LEAKY.to_string()) && texts.contains(&changed));
    assert!(files_holding(dir.path(), NEEDLES, &["config.toml"]).is_empty());

    // A reset sets the current one aside too.
    rotate_config_backup(dir.path()).unwrap();
    assert!(!dir.path().join("config.toml.backup.enc").exists());
    assert_eq!(copies(dir.path()).len(), 2);
}

/// With no key to encrypt under, no copy is written, rather than a clear one.
#[test]
fn without_a_key_no_snapshot_is_written() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), LEAKY).unwrap();
    assert!(backup_config_sealed(dir.path()).is_err());
    assert_eq!(names(dir.path()), ["config.toml"]);
    // No config.toml: nothing to copy, not an error.
    let empty = tempfile::tempdir().unwrap();
    backup_config_sealed(empty.path()).unwrap();
}

/// A snapshot left by 0.15.x (clear, scrubbed by name or not) is converted once
/// at start: sealed under the key in use, the clear file gone, no temp left, and
/// a second pass changes nothing.
#[test]
fn existing_clear_copies_are_sealed_once_at_start() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let rotated = "config.toml.backup.20261005T000000Z";
    let corrupt = "config.toml.corrupt.20261005T000000Z";
    let retired = "config.toml.retired-key.20261005T000000Z";
    for name in ["config.toml.backup", rotated, corrupt, retired] {
        std::fs::write(d.join(name), LEAKY).unwrap();
    }
    std::fs::write(d.join(".config.toml.backup.tmp"), LEAKY).unwrap();
    std::fs::write(d.join(".config.toml.backup.rotate.tmp"), LEAKY).unwrap();
    std::fs::write(d.join("config.toml"), "[server]\nport = 1\n").unwrap();

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(report.sealed, 4, "{report:?}");
    assert_eq!(report.temps, 2, "{report:?}");
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(files_holding(d, NEEDLES, &["config.toml"]).is_empty());
    for name in [
        "config.toml.backup.enc",
        &format!("{rotated}.enc"),
        &format!("{corrupt}.enc"),
        &format!("{retired}.enc"),
    ] {
        assert_eq!(
            open_sealed(&d.join(name), std::slice::from_ref(&key)).unwrap(),
            LEAKY,
            "{name}"
        );
    }
    let after = names(d);
    assert_eq!(protect_clear_copies(d, Some(&key)), Protected::default());
    assert_eq!(names(d), after);
}

/// A copy that cannot be written (full disk) stays as it is, key line and all,
/// and the next start converts it; it is never deleted for the failure.
#[test]
fn a_copy_that_cannot_be_sealed_is_kept_and_retried() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let text = format!(
        "encryption_secret = \"{}\"\n{LEAKY}",
        crypto::generate_secret()
    );
    std::fs::write(d.join("config.toml.backup"), &text).unwrap();
    // The temp file of the write cannot be created.
    std::fs::create_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        text
    );
    assert!(!d.join("config.toml.backup.enc").exists());

    std::fs::remove_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!((report.sealed, report.failures.len()), (1, 0), "{report:?}");
    assert!(!d.join("config.toml.backup").exists());
    assert_eq!(
        open_sealed(&d.join("config.toml.backup.enc"), &[key]).unwrap(),
        text
    );
}

/// With no key to seal under, a clear copy is cut down to its key line, or
/// dropped when it has none; never kept whole. A second pass changes nothing.
#[test]
fn without_a_key_a_clear_copy_keeps_only_its_key_line() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let k = crypto::generate_secret();
    // The key line of a file the parser rejects as a whole is found line by line.
    std::fs::write(
        d.join("config.toml.backup"),
        format!("encryption_secret = \"{k}\"\n{LEAKY}[broken\n"),
    )
    .unwrap();
    std::fs::write(d.join("config.toml.backup.20261001T000000Z"), LEAKY).unwrap();
    std::fs::write(
        d.join("config.toml.corrupt.20261001T000000Z"),
        format!("{BROKEN}encryption_secret = \"{k}\"\n"),
    )
    .unwrap();

    let report = protect_clear_copies(d, None);
    assert_eq!(report.reduced, 3, "{report:?}");
    assert!(files_holding(d, NEEDLES, &[]).is_empty());
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        format!("encryption_secret = \"{k}\"\n")
    );
    assert!(!d.join("config.toml.backup.20261001T000000Z").exists());
    assert!(d.join("config.toml.corrupt.20261001T000000Z").exists());
    let after = names(d);
    assert_eq!(protect_clear_copies(d, None), Protected::default());
    assert_eq!(names(d), after);
}

/// The kept-aside content of an unreadable config.toml: whole but encrypted
/// when a key store holds a key, else the key line alone.
#[test]
fn an_unreadable_config_is_set_aside_encrypted_or_reduced_to_its_key() {
    let (dir, key) = dir_with_key();
    let aside = set_aside_content(dir.path(), BROKEN.as_bytes(), Some(&key)).unwrap();
    let Aside::Sealed(name) = aside else {
        panic!("{aside:?}")
    };
    assert!(
        name.starts_with(CORRUPT_PREFIX) && name.ends_with(".enc"),
        "{name}"
    );
    assert_eq!(
        open_sealed(&dir.path().join(&name), &[key]).unwrap(),
        BROKEN
    );
    assert!(files_holding(dir.path(), NEEDLES, &[]).is_empty());

    let bare = tempfile::tempdir().unwrap();
    let k = crypto::generate_secret();
    let text = format!("{LEAKY}encryption_secret = \"{k}\"\n[broken");
    let Aside::KeyOnly(name) = set_aside_content(bare.path(), text.as_bytes(), None).unwrap()
    else {
        panic!("a key line is kept")
    };
    assert_eq!(
        std::fs::read_to_string(bare.path().join(name)).unwrap(),
        format!("encryption_secret = \"{k}\"\n")
    );
    assert_eq!(
        set_aside_content(bare.path(), b"x = [broken", None).unwrap(),
        Aside::Nothing
    );
    assert!(files_holding(bare.path(), NEEDLES, &[]).is_empty());
}

/// What counts as a copy: the temp files of the writers, the encrypted
/// pre-credential-store backup and the shelter are not.
#[test]
fn copies_are_told_from_other_files() {
    assert_eq!(classify("config.toml.backup"), Some((Kind::Backup, false)));
    assert_eq!(
        classify("config.toml.backup.enc"),
        Some((Kind::Backup, true))
    );
    assert_eq!(
        classify("config.toml.backup.20261005T000000Z-1.enc"),
        Some((Kind::Backup, true))
    );
    assert_eq!(
        classify("config.toml.corrupt.20261005T000000Z"),
        Some((Kind::Corrupt, false))
    );
    assert_eq!(
        classify("config.toml.retired-key.1.enc"),
        Some((Kind::RetiredKey, true))
    );
    for other in [
        "config.toml",
        ".config.toml.backup.enc.tmp",
        "config.toml.pre-credential-store.enc",
        "config.toml.preserved.enc",
    ] {
        assert_eq!(classify(other), None, "{other}");
    }
}

/// A key change (a retirement, a restore) moves every sealed copy under the
/// new key, so none is stranded under a key that is gone.
#[test]
fn sealed_copies_follow_a_key_change() {
    let (dir, old) = dir_with_key();
    let new = crypto::generate_secret();
    let names = [
        "config.toml.backup.enc",
        "config.toml.backup.20261005T000000Z.enc",
        "config.toml.corrupt.20261005T000000Z.enc",
        "config.toml.retired-key.20261005T000000Z.enc",
    ];
    for name in names {
        write_sealed(dir.path(), &dir.path().join(name), LEAKY, &old).unwrap();
    }
    assert!(crate::core::credential_store::reencrypt_backup(dir.path(), &old, &new).unwrap());
    for name in names {
        let path = dir.path().join(name);
        assert_eq!(
            open_sealed(&path, std::slice::from_ref(&new)).unwrap(),
            LEAKY
        );
        assert!(
            open_sealed(&path, std::slice::from_ref(&old)).is_err(),
            "{name}"
        );
    }

    // A retirement that stopped half-way is finished at the next start.
    let stranded = dir.path().join("config.toml.backup.1.enc");
    write_sealed(dir.path(), &stranded, LEAKY, &old).unwrap();
    let candidates = vec![(old.clone(), "sidecar")];
    assert!(
        crate::core::credential_store::reencrypt_backup_to(dir.path(), &candidates, &new).unwrap()
    );
    assert_eq!(open_sealed(&stranded, &[new]).unwrap(), LEAKY);
}

/// KT-1528 — every writer of a copy of config.toml, listed, run on a file
/// holding every credential shape: no file of the data directory but
/// config.toml holds a clear value afterwards, the temp files included.
///
/// Writers: `backup_config_sealed` (`config.toml.backup.enc`), its rotation and
/// `rotate_config_backup` (a reset), `protect_clear_copies` (the conversion at
/// start of what 0.15.x wrote, and the cut down without a key),
/// `set_aside_content` (`config.toml.corrupt.<ts>[.enc]`, called by
/// `config::load`), `keystore::set_aside_retired_key`
/// (`config.toml.retired-key.<ts>.enc`), `credential_store::reencrypt_backup`
/// (a key change), and the two files that were already encrypted
/// (`config.toml.pre-credential-store.enc`, `config.toml.preserved.enc`). The
/// data export serialises the typed config, not the file. Each one has its own
/// test in its module; this one runs them side by side.
#[test]
fn no_writer_of_a_config_copy_leaves_a_clear_value() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    std::fs::write(d.join("config.toml"), LEAKY).unwrap();
    // What 0.15.x left, in clear.
    std::fs::write(d.join("config.toml.backup"), BROKEN).unwrap();
    std::fs::write(d.join("config.toml.corrupt.20261001T000000Z"), LEAKY).unwrap();
    std::fs::write(d.join(".config.toml.backup.rotate.tmp"), LEAKY).unwrap();

    backup_config_sealed(d).unwrap();
    std::fs::write(d.join("config.toml"), format!("{LEAKY}# changed\n")).unwrap();
    backup_config_sealed(d).unwrap();
    rotate_config_backup(d).unwrap();
    set_aside_content(d, BROKEN.as_bytes(), Some(&key)).unwrap();
    set_aside_content(d, LEAKY.as_bytes(), None).unwrap();
    protect_clear_copies(d, Some(&key));
    let other = crypto::generate_secret();
    crate::core::credential_store::reencrypt_backup(d, &key, &other).unwrap();
    crate::core::credential_store::reencrypt_backup(d, &other, &key).unwrap();

    let leaks = files_holding(d, NEEDLES, &["config.toml"]);
    assert!(leaks.is_empty(), "clear values in {leaks:?}");
    assert!(
        names(d).iter().all(|n| n == "config.toml"
            || n == "encryption_key"
            || n.ends_with(".enc")
            || classify(n).is_some_and(|(kind, _)| kind == Kind::Corrupt)),
        "{:?}",
        names(d)
    );
}

/// A data directory whose key file cannot be read (a directory stands where it
/// is): the stores hold a key that cannot be told from "no key".
fn dir_with_unreadable_key_store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(keyvault::SIDECAR_FILENAME)).unwrap();
    dir
}

fn bytes_of(dir: &Path, names: &[&str]) -> Vec<Vec<u8>> {
    names
        .iter()
        .map(|n| std::fs::read(dir.join(n)).unwrap())
        .collect()
}

/// KT-1528 — a key store that cannot be read (locked keychain, I/O error) is
/// not "no key": every clear copy is left exactly as it is, so a transient lock
/// destroys neither a setting nor a key line. A store that reads and holds
/// nothing proves there is none: then the copies are cut down. Once a store
/// reads a key, the untouched copies are sealed whole.
#[test]
fn an_unreadable_key_store_leaves_every_copy_untouched_until_it_reads() {
    let dir = dir_with_unreadable_key_store();
    let d = dir.path();
    let k = crypto::generate_secret();
    let files = [
        (
            "config.toml.backup",
            format!("encryption_secret = \"{k}\"\n{LEAKY}"),
        ),
        ("config.toml.backup.20261001T000000Z", LEAKY.to_string()),
        ("config.toml.corrupt.20261001T000000Z", BROKEN.to_string()),
        (
            "config.toml.retired-key.20261001T000000Z",
            "encryption_secret = \"not-a-key\"\n".to_string(),
        ),
    ];
    for (name, text) in &files {
        std::fs::write(d.join(name), text).unwrap();
    }
    let names_in: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
    let before = bytes_of(d, &names_in);

    let state = sealing(d, false);
    assert!(matches!(state, Sealing::Unavailable(_)), "{state:?}");
    let report = protect_copies(d, &state);
    assert_eq!((report.deferred, report.reduced, report.sealed), (4, 0, 0));
    // The migration's rotation path takes the same care.
    rotate_config_backup(d).unwrap();
    assert_eq!(bytes_of(d, &names_in), before);
    assert!(!d.join("config.toml.backup.enc").exists());

    // The same copies with a store that reads and holds nothing: proven absent.
    let bare = tempfile::tempdir().unwrap();
    for (name, text) in &files[..3] {
        std::fs::write(bare.path().join(name), text).unwrap();
    }
    assert_eq!(sealing(bare.path(), false), Sealing::Absent);
    let report = protect_copies(bare.path(), &Sealing::Absent);
    assert_eq!(report.reduced, 3, "{report:?}");
    assert!(files_holding(bare.path(), NEEDLES, &[]).is_empty());

    // The store reads again: the untouched copies are sealed whole.
    std::fs::remove_dir(d.join(keyvault::SIDECAR_FILENAME)).unwrap();
    let key = crypto::generate_secret();
    SidecarFile::in_dir(d).store(&key).unwrap();
    let report = protect_copies(d, &sealing(d, false));
    assert_eq!((report.sealed, report.deferred), (4, 0), "{report:?}");
    assert!(files_holding(d, NEEDLES, &[]).is_empty());
    for (name, text) in &files {
        let sealed = d.join(format!("{name}.enc"));
        assert_eq!(
            open_sealed(&sealed, std::slice::from_ref(&key)).unwrap(),
            *text
        );
    }
}

/// An unreadable config.toml met while the key stores cannot be read is moved
/// as it is (owner-only), not dropped nor reduced; the next start seals it.
#[test]
fn an_unreadable_config_met_with_an_unreadable_key_store_is_moved_whole() {
    let dir = dir_with_unreadable_key_store();
    let d = dir.path();
    let cfg = d.join("config.toml");
    let text = format!(
        "{BROKEN}encryption_secret = \"{}\"\n",
        crypto::generate_secret()
    );
    std::fs::write(&cfg, &text).unwrap();

    let aside = set_aside_unchanged(d, &cfg).unwrap();
    let Aside::Deferred(name) = aside else {
        panic!("{aside:?}")
    };
    assert!(!cfg.exists());
    assert_eq!(std::fs::read_to_string(d.join(&name)).unwrap(), text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(d.join(&name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    std::fs::remove_dir(d.join(keyvault::SIDECAR_FILENAME)).unwrap();
    let key = crypto::generate_secret();
    SidecarFile::in_dir(d).store(&key).unwrap();
    let report = protect_copies(d, &sealing(d, false));
    assert_eq!(report.sealed, 1, "{report:?}");
    assert!(files_holding(d, NEEDLES, &[]).is_empty());
    assert_eq!(
        open_sealed(&d.join(format!("{name}.enc")), &[key]).unwrap(),
        text
    );
}

/// What is sealed is the raw bytes: a byte that is not UTF-8 survives the
/// round trip, in the conversion of a clear copy and in the set-aside file.
#[test]
fn sealing_keeps_the_raw_bytes() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let mut raw = b"pat = \"pat-clear\"\n# \xff\xfe\x80 not utf-8\n".to_vec();
    raw.extend_from_slice(b"[tokens]\nmistral = \"sk-mistral-clear\"\n");
    std::fs::write(d.join("config.toml.backup"), &raw).unwrap();

    assert_eq!(protect_clear_copies(d, Some(&key)).sealed, 1);
    assert_eq!(
        open_sealed_bytes(
            &d.join("config.toml.backup.enc"),
            std::slice::from_ref(&key)
        )
        .unwrap(),
        raw
    );
    assert!(files_holding(d, NEEDLES, &[]).is_empty());

    let Aside::Sealed(name) = set_aside_content(d, &raw, Some(&key)).unwrap() else {
        panic!("sealed")
    };
    assert_eq!(open_sealed_bytes(&d.join(name), &[key]).unwrap(), raw);
}

/// A clear retired-key file holding a value that is not a key is never dropped
/// for lack of a key: kept until one exists, then sealed whole. Only a file
/// proven empty goes.
#[test]
fn a_retired_key_file_holding_a_non_key_value_is_sealed_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let value = "encryption_secret = \"hunter2-not-a-key\"\n";
    let kept = "config.toml.retired-key.20261001T000000Z";
    let empty = "config.toml.retired-key.20261002T000000Z";
    std::fs::write(d.join(kept), value).unwrap();
    std::fs::write(d.join(empty), "  \n").unwrap();

    let report = protect_clear_copies(d, None);
    assert_eq!((report.deferred, report.reduced), (1, 1), "{report:?}");
    assert_eq!(std::fs::read_to_string(d.join(kept)).unwrap(), value);
    assert!(!d.join(empty).exists());

    let key = crypto::generate_secret();
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(report.sealed, 1, "{report:?}");
    assert!(!d.join(kept).exists());
    assert_eq!(
        open_sealed(&d.join(format!("{kept}.enc")), &[key]).unwrap(),
        value
    );
}

/// The kind of copy is bound to the ciphertext: a sealed copy renamed or
/// swapped for another kind does not open, though the key is the right one.
#[test]
fn a_sealed_copy_does_not_open_as_another_kind() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let backup = d.join("config.toml.backup.enc");
    write_sealed(d, &backup, b"a = 1\n", &key).unwrap();
    let keys = std::slice::from_ref(&key);
    for as_kind in [
        "config.toml.corrupt.20261001T000000Z.enc",
        "config.toml.retired-key.20261001T000000Z.enc",
    ] {
        std::fs::copy(&backup, d.join(as_kind)).unwrap();
        assert!(open_sealed(&d.join(as_kind), keys).is_err(), "{as_kind}");
    }
    // A corrupt copy swapped in for a backup does not open either.
    let (dir2, _) = dir_with_key();
    let corrupt = dir2.path().join("config.toml.corrupt.20261001T000000Z.enc");
    write_sealed(dir2.path(), &corrupt, b"a = 1\n", &key).unwrap();
    let swapped = d.join("config.toml.backup.20261001T000000Z.enc");
    std::fs::copy(&corrupt, &swapped).unwrap();
    assert!(open_sealed(&swapped, keys).is_err());
    // The right kind still opens; a ciphertext without the associated data is
    // not accepted as a copy.
    assert_eq!(open_sealed(&backup, keys).unwrap(), "a = 1\n");
    let plain = crypto::encrypt("a = 1\n", &crypto::parse_secret(&key).unwrap()).unwrap();
    assert!(open_text(&plain, keys, Kind::Backup).is_none());
}

/// KT-1528 — a config.toml holding a valid key line next to a byte that is not
/// UTF-8 still yields that key: with every store empty (a locked boot without
/// vault keys) the clear copies are sealed under it, not cut down or dropped.
#[test]
fn a_non_utf8_config_toml_still_gives_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let key = crypto::generate_secret();
    let mut raw = format!("encryption_secret = \"{key}\"\n# ").into_bytes();
    raw.extend_from_slice(b"\xff\xfe\x80 not utf-8\n");
    std::fs::write(d.join("config.toml"), &raw).unwrap();
    std::fs::write(d.join("config.toml.backup"), LEAKY).unwrap();
    std::fs::write(d.join("config.toml.backup.1"), &raw).unwrap();
    std::fs::write(
        d.join("config.toml.corrupt.20261001T000000Z"),
        BROKEN.as_bytes(),
    )
    .unwrap();

    // The stores read and are empty: the file's own key is the only one.
    assert_eq!(sealing(d, false), Sealing::Absent);
    let state = sealing(d, true);
    assert_eq!(state, Sealing::Key(key.clone()));
    let report = protect_copies(d, &state);
    assert_eq!((report.sealed, report.reduced, report.deferred), (3, 0, 0));
    assert!(files_holding(d, NEEDLES, &["config.toml"]).is_empty());
    assert_eq!(
        open_sealed_bytes(
            &d.join("config.toml.backup.1.enc"),
            std::slice::from_ref(&key)
        )
        .unwrap(),
        raw
    );
    assert_eq!(
        open_sealed(&d.join("config.toml.backup.enc"), &[key]).unwrap(),
        LEAKY
    );
}

/// KT-1528 — the key a config.toml holds does not stand in for a key store that
/// cannot be read: it may be an older key than the live one.
#[test]
fn the_files_key_does_not_stand_in_for_an_unreadable_store() {
    let dir = dir_with_unreadable_key_store();
    let d = dir.path();
    let old = crypto::generate_secret();
    std::fs::write(
        d.join("config.toml"),
        format!("encryption_secret = \"{old}\"\n"),
    )
    .unwrap();
    std::fs::write(d.join("config.toml.backup"), LEAKY).unwrap();

    let state = sealing(d, true);
    assert!(matches!(state, Sealing::Unavailable(_)), "{state:?}");
    assert_eq!(sealing_key(d, true), None);
    assert_eq!(protect_copies(d, &state).deferred, 1);
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        LEAKY
    );
    assert!(!d.join("config.toml.backup.enc").exists());
}

/// KT-1528 — a dangling symlink holds its name: `free_name` skips it, so a new
/// copy never lands on it.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn free_name_skips_a_dangling_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::os::unix::fs::symlink(d.join("nowhere"), d.join("config.toml.corrupt.1")).unwrap();
    assert!(!d.join("config.toml.corrupt.1").exists());
    assert_eq!(
        free_name(d, "config.toml.corrupt.1", ""),
        "config.toml.corrupt.1-1"
    );
}

/// KT-1528 — a symlinked config.toml is refused (its target is not Kronn's to
/// chmod or move) and the target keeps its mode; a plain file is made
/// owner-only before the move, not after.
#[cfg(unix)] // creates symlinks and asserts POSIX mode bits
#[test]
fn a_symlinked_config_is_not_set_aside_and_its_target_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let target = d.join("elsewhere.toml");
    std::fs::write(&target, "x = 1\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cfg = d.join("config.toml");
    std::os::unix::fs::symlink(&target, &cfg).unwrap();

    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(format!("{err:#}").contains("symbolic link"), "{err:#}");
    assert!(cfg.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(copies(d).is_empty());

    // A plain file readable by others: owner-only once moved.
    std::fs::remove_file(&cfg).unwrap();
    std::fs::write(&cfg, "x = 2\n").unwrap();
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let Aside::Deferred(name) = set_aside_unchanged(d, &cfg).unwrap() else {
        panic!("deferred")
    };
    assert_eq!(
        std::fs::metadata(d.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

/// KT-1528 — the chmod goes through an `O_NOFOLLOW` handle: a dangling link and
/// a path that is no regular file are refused, and nothing is moved.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn the_owner_only_step_refuses_a_link_and_a_non_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = d.join("config.toml");

    std::os::unix::fs::symlink(d.join("nowhere"), &cfg).unwrap();
    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(format!("{err:#}").contains("symbolic link"), "{err:#}");
    assert!(cfg.symlink_metadata().unwrap().file_type().is_symlink());
    std::fs::remove_file(&cfg).unwrap();

    std::fs::create_dir(&cfg).unwrap();
    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    assert!(cfg.is_dir());
    assert_eq!(names(d), vec!["config.toml".to_string()]);
}

/// KT-1528 — the path swapped for a link between the open and the rename: what
/// was moved is the link, not the opened file, so it is moved back and the
/// set-aside is refused. Nothing is chmodded or lost, and no copy is left.
#[cfg(unix)] // creates symlinks and asserts POSIX mode bits
#[test]
fn a_path_swapped_for_a_link_while_being_set_aside_is_moved_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = d.join("config.toml");
    std::fs::write(&cfg, "auth_token = \"secret-1\"\n").unwrap();
    let outside = d.join("elsewhere.toml");
    std::fs::write(&outside, "x = 1\n").unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();

    let (swap_cfg, swap_outside) = (cfg.clone(), outside.clone());
    BEFORE_RENAME.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(&swap_cfg).unwrap();
            std::os::unix::fs::symlink(&swap_outside, &swap_cfg).unwrap();
        }));
    });
    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(
        format!("{err:#}").contains("changed while it was being set aside"),
        "{err:#}"
    );
    assert!(
        cfg.symlink_metadata().unwrap().file_type().is_symlink(),
        "the swapped entry is back where it was"
    );
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644,
        "the link's target keeps its mode"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "x = 1\n");
    assert_eq!(
        names(d),
        vec!["config.toml".to_string(), "elsewhere.toml".to_string()],
        "no corrupt copy is left"
    );
}

/// KT-1528 — the same refusal when the path is swapped for another regular
/// file (a different inode): it is moved back untouched.
#[cfg(unix)] // swaps the file inside the libc openat/renameat move
#[test]
fn a_path_swapped_for_another_file_while_being_set_aside_is_moved_back() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = d.join("config.toml");
    std::fs::write(&cfg, "x = 1\n").unwrap();

    let swap_cfg = cfg.clone();
    BEFORE_RENAME.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(&swap_cfg).unwrap();
            std::fs::write(&swap_cfg, "x = 2\n").unwrap();
        }));
    });
    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(
        format!("{err:#}").contains("changed while it was being set aside"),
        "{err:#}"
    );
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "x = 2\n");
    assert_eq!(names(d), vec!["config.toml".to_string()]);
}

/// KT-1528 — the move-back never replaces: a config.toml created at the original
/// path after the swap was found is left alone, and what was moved aside stays in
/// its copy file, so both survive and the error says so.
#[cfg(unix)] // swaps the file inside the libc openat/renameat move
#[test]
fn a_config_recreated_before_the_move_back_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cfg = d.join("config.toml");
    std::fs::write(&cfg, "x = 1\n").unwrap();

    let swap_cfg = cfg.clone();
    BEFORE_RENAME.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(&swap_cfg).unwrap();
            std::fs::write(&swap_cfg, "x = 2\n").unwrap();
        }));
    });
    let recreate = cfg.clone();
    BEFORE_MOVE_BACK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            std::fs::write(&recreate, "x = 3\n").unwrap();
        }));
    });
    let err = set_aside_unchanged(d, &cfg).unwrap_err();
    assert!(
        format!("{err:#}").contains("nothing was overwritten"),
        "{err:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        "x = 3\n",
        "the file created in the meantime is untouched"
    );
    let kept: Vec<String> = names(d)
        .into_iter()
        .filter(|n| n.starts_with("config.toml.corrupt."))
        .collect();
    assert_eq!(kept.len(), 1, "the moved entry is kept: {kept:?}");
    assert_eq!(
        std::fs::read_to_string(d.join(&kept[0])).unwrap(),
        "x = 2\n"
    );
}

/// KT-1528 — the rename itself refuses to replace an existing destination.
#[cfg(all(
    unix,
    any(target_os = "linux", target_os = "android", target_os = "macos")
))] // libc renameat2/renameatx_np
#[test]
fn rename_noreplace_keeps_both_files_when_the_destination_exists() {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("a"), "a").unwrap();
    std::fs::write(d.join("b"), "b").unwrap();
    let handle = std::fs::File::open(d).unwrap();
    let (a, b, c) = (
        CString::new("a").unwrap(),
        CString::new("b").unwrap(),
        CString::new("c").unwrap(),
    );
    let err = rename_noreplace(handle.as_raw_fd(), &a, &b).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
    assert_eq!(std::fs::read_to_string(d.join("a")).unwrap(), "a");
    assert_eq!(std::fs::read_to_string(d.join("b")).unwrap(), "b");
    rename_noreplace(handle.as_raw_fd(), &a, &c).unwrap();
    assert!(!d.join("a").exists());
    assert_eq!(std::fs::read_to_string(d.join("c")).unwrap(), "a");
}

/// KT-1528 — a path outside the directory handed in is refused, not set aside by
/// its bare file name from the wrong place.
#[test]
fn a_path_outside_the_data_directory_is_not_set_aside() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let other = root.path().join("other");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&other).unwrap();
    std::fs::write(data.join("config.toml"), "in data\n").unwrap();
    std::fs::write(other.join("config.toml"), "in other\n").unwrap();

    let err = set_aside_unchanged(&data, &other.join("config.toml")).unwrap_err();
    assert!(format!("{err:#}").contains("is not in"), "{err:#}");
    assert_eq!(names(&data), vec!["config.toml".to_string()]);
    assert_eq!(names(&other), vec!["config.toml".to_string()]);
    assert_eq!(
        std::fs::read_to_string(other.join("config.toml")).unwrap(),
        "in other\n"
    );
}

/// KT-1528 — a data directory that is itself a symlink (dotfiles setups) still
/// works: only the file is refused when it is a link.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_symlinked_data_directory_can_still_be_set_aside_from() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let linked = root.path().join("linked");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    std::fs::write(real.join("config.toml"), "x = 1\n").unwrap();

    let Aside::Deferred(name) = set_aside_unchanged(&linked, &linked.join("config.toml")).unwrap()
    else {
        panic!("deferred")
    };
    assert!(!real.join("config.toml").exists());
    assert_eq!(std::fs::read_to_string(real.join(name)).unwrap(), "x = 1\n");
}

/// KT-1528 — a dangling `config.toml.backup.enc` link is neither skipped nor
/// replaced: the rotation refuses it, and so does the snapshot writer, which
/// would otherwise write over it.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_dangling_backup_link_is_refused_not_replaced() {
    let (dir, _key) = dir_with_key();
    let d = dir.path();
    std::fs::write(d.join("config.toml"), "language = \"fr\"\n").unwrap();
    let link = d.join("config.toml.backup.enc");
    std::os::unix::fs::symlink(d.join("nowhere"), &link).unwrap();
    assert!(!link.exists());

    let err = rotate_sealed_backup(d).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
    let err = rotate_config_backup(d).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
    let err = backup_config_sealed(d).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");

    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        d.join("nowhere"),
        "the link is untouched"
    );
    assert!(
        names(d)
            .iter()
            .all(|n| !n.starts_with("config.toml.backup.2")),
        "no rotation was made of the link: {:?}",
        names(d)
    );
    assert!(!d.join("nowhere").exists());
}

/// KT-1528 — a link to a real file is refused as well, and its target is not
/// read as the snapshot nor rotated.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_backup_link_to_a_real_file_is_refused_too() {
    let (dir, _key) = dir_with_key();
    let d = dir.path();
    std::fs::write(d.join("config.toml"), "language = \"fr\"\n").unwrap();
    let real = d.join("elsewhere.enc");
    std::fs::write(&real, "not Kronn's").unwrap();
    let link = d.join("config.toml.backup.enc");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert!(rotate_sealed_backup(d).is_err());
    assert!(backup_config_sealed(d).is_err());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "not Kronn's");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
}

/// KT-1528 — a missing snapshot is no error for the rotation.
#[test]
fn rotating_a_missing_backup_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    rotate_sealed_backup(dir.path()).unwrap();
    assert!(names(dir.path()).is_empty());
}

/// KT-1528 — a unix without an atomic no-replace rename fails closed: nothing is
/// renamed, whether or not the destination exists.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
#[test]
fn rename_noreplace_fails_closed_without_an_atomic_call() {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("a"), "a").unwrap();
    let handle = std::fs::File::open(d).unwrap();
    let (a, c) = (CString::new("a").unwrap(), CString::new("c").unwrap());
    let err = rename_noreplace(handle.as_raw_fd(), &a, &c).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOTSUP));
    assert_eq!(names(d), vec!["a".to_string()]);
}

/// KT-1528 — Windows: the move never replaces an existing destination, refuses a
/// path outside the directory, and moves a plain file.
#[cfg(windows)]
#[test]
fn windows_set_aside_never_replaces_and_stays_in_the_directory() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let other = root.path().join("other");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&other).unwrap();
    let cfg = data.join("config.toml");
    std::fs::write(&cfg, "in data\n").unwrap();
    std::fs::write(other.join("config.toml"), "in other\n").unwrap();

    // Outside the directory: refused, both files untouched.
    let err = move_owner_only_no_follow(&data, &other.join("config.toml"), "aside").unwrap_err();
    assert!(format!("{err:#}").contains("is not in"), "{err:#}");
    assert_eq!(
        std::fs::read_to_string(other.join("config.toml")).unwrap(),
        "in other\n"
    );

    // The destination exists: refused, both files kept, nothing overwritten.
    std::fs::write(data.join("taken"), "taken\n").unwrap();
    let err = move_owner_only_no_follow(&data, &cfg, "taken").unwrap_err();
    assert!(
        format!("{err:#}").contains("nothing was overwritten"),
        "{err:#}"
    );
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "in data\n");
    assert_eq!(
        std::fs::read_to_string(data.join("taken")).unwrap(),
        "taken\n"
    );

    // A free name: moved.
    move_owner_only_no_follow(&data, &cfg, "aside").unwrap();
    assert!(!cfg.exists());
    assert_eq!(
        std::fs::read_to_string(data.join("aside")).unwrap(),
        "in data\n"
    );

    // A directory is not a regular file.
    std::fs::create_dir(&cfg).unwrap();
    let err = move_owner_only_no_follow(&data, &cfg, "other-aside").unwrap_err();
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    assert!(cfg.is_dir());
}
