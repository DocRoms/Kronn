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

/// A copy that cannot be sealed (full disk) is cut down to its key line, never
/// kept whole; the next start seals what is left.
#[test]
fn a_copy_that_cannot_be_sealed_is_cut_down_and_retried() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let k = crypto::generate_secret();
    let text = format!("encryption_secret = \"{k}\"\n{LEAKY}");
    std::fs::write(d.join("config.toml.backup"), &text).unwrap();
    // The temp file of the write cannot be created.
    std::fs::create_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(
        (report.reduced, report.failures.len()),
        (1, 0),
        "{report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        format!("encryption_secret = \"{k}\"\n")
    );
    assert!(files_holding(d, NEEDLES, &[]).is_empty());
    assert!(!d.join("config.toml.backup.enc").exists());

    std::fs::remove_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!((report.sealed, report.failures.len()), (1, 0), "{report:?}");
    assert!(!d.join("config.toml.backup").exists());
    assert_eq!(
        open_sealed(
            &d.join("config.toml.backup.enc"),
            std::slice::from_ref(&key)
        )
        .unwrap(),
        format!("encryption_secret = \"{k}\"\n")
    );
    assert_eq!(protect_clear_copies(d, Some(&key)), Protected::default());
}

/// A copy that can be neither sealed nor cut down stays as it is and is
/// reported (the boot never panics); the next start retries.
#[test]
fn a_copy_that_can_be_neither_sealed_nor_cut_down_is_left_and_reported() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let text = format!(
        "encryption_secret = \"{}\"\n{LEAKY}",
        crypto::generate_secret()
    );
    std::fs::write(d.join("config.toml.backup"), &text).unwrap();
    // Both writes use the same temp name when a directory holds it.
    std::fs::create_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();
    std::fs::create_dir(d.join(".config.toml.backup.tmp")).unwrap();

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        text
    );
}

/// A copy whose seal fails and that cannot be cut down to a key line stays as it
/// is, reported in `failures` with a true message, never counted as cut down or
/// deferred.
#[test]
fn a_failed_seal_that_cannot_be_cut_down_is_a_failure_not_a_cut_down() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let junk = format!("encryption_secret = \"not-a-key\"\n{LEAKY}");
    std::fs::write(d.join("config.toml.backup"), &junk).unwrap();
    // A retired-key copy holding a value that is no key.
    let retired = "config.toml.retired-key.20261001T000000Z";
    std::fs::write(d.join(retired), "an-old-value\n").unwrap();
    // A copy that already holds nothing but its key line.
    let only = format!("encryption_secret = \"{}\"\n", crypto::generate_secret());
    let rotated = "config.toml.backup.20261001T000000Z";
    std::fs::write(d.join(rotated), &only).unwrap();
    for name in ["config.toml.backup", retired, rotated] {
        std::fs::create_dir(d.join(format!(".{name}.enc.tmp"))).unwrap();
    }

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(report.failures.len(), 3, "{report:?}");
    assert_eq!(
        (report.reduced, report.deferred, report.sealed),
        (0, 0, 0),
        "{report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        junk
    );
    assert_eq!(
        std::fs::read_to_string(d.join(retired)).unwrap(),
        "an-old-value\n"
    );
    assert_eq!(std::fs::read_to_string(d.join(rotated)).unwrap(), only);
}

/// A key line the parser rejects as a whole (duplicate key) is found past a
/// value that is not a key; a copy whose `encryption_secret` is no key at all is
/// never dropped.
#[test]
fn a_valid_key_line_behind_a_junk_one_is_kept_and_a_junk_key_is_never_dropped() {
    let k = crypto::generate_secret();
    // Duplicate top-level key: the whole text does not parse, the top-level lines
    // are scanned (the second one with another value between).
    let dup = format!("encryption_secret = \"junk\"\nencryption_secret = \"{k}\"\n{LEAKY}");
    let apart = format!(
        "encryption_secret = \"junk\"\nlanguage = \"fr\"\nencryption_secret = \"{k}\"\n{LEAKY}"
    );
    for text in [&dup, &apart] {
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert_eq!(key_of(text), Some(k.clone()), "{text}");
        assert_eq!(config::salvage_key_line(text), Some(k.clone()), "{text}");
    }
    // Without a real key, the first value is still what is salvaged.
    assert_eq!(
        config::salvage_key_line("encryption_secret = \"junk\"\n"),
        Some("junk".to_string())
    );

    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("config.toml.backup"), &dup).unwrap();
    let junk = format!("encryption_secret = \"junk\"\n{LEAKY}");
    std::fs::write(d.join("config.toml.corrupt.20261001T000000Z"), &junk).unwrap();

    // The valid key is found past the junk one (above), but a copy holding a
    // value it cannot read is not cut down to the key line: the junk value may be
    // the only trace of another key. Both are kept whole and reported.
    let report = protect_clear_copies(d, None);
    assert_eq!(
        (report.reduced, report.failures.len()),
        (0, 2),
        "{report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
        dup
    );
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.corrupt.20261001T000000Z")).unwrap(),
        junk
    );
}

/// An `encryption_secret` that is not a top-level line of the document (under a
/// table, in a multi-line string, after the first table of a text that does not
/// parse) is a value of something else: it is never the key, never kept as the
/// key-only copy, never a key to seal under.
#[test]
fn an_encryption_secret_below_the_top_level_is_never_the_key() {
    let section_value = crypto::generate_secret();
    let top = crypto::generate_secret();

    // Parses: only the top-level value decides.
    let parsed = format!(
        "encryption_secret = \"junk\"\n[tokens]\nencryption_secret = \"{section_value}\"\n"
    );
    assert!(parsed.parse::<toml::Table>().is_ok());
    assert_eq!(key_of(&parsed), None);
    let only_section =
        format!("language = \"fr\"\n[tokens]\nencryption_secret = \"{section_value}\"\n");
    assert_eq!(key_of(&only_section), None);
    // The top-level key wins over a section value.
    let both = format!(
        "encryption_secret = \"{top}\"\n[tokens]\nencryption_secret = \"{section_value}\"\n"
    );
    assert_eq!(key_of(&both), Some(top.clone()));

    // Does not parse (duplicate top-level key, then a table): the scan stops at
    // the first table, a key line after it is ignored.
    let after_table = format!(
        "encryption_secret = \"junk\"\nencryption_secret = \"junk2\"\n[tokens]\n\
         encryption_secret = \"{section_value}\"\n"
    );
    assert!(after_table.parse::<toml::Table>().is_err());
    assert_eq!(key_of(&after_table), None);
    assert_eq!(
        config::salvage_key_line(&after_table),
        Some("junk".to_string())
    );
    let array_table = format!(
        "encryption_secret = \"junk\"\nencryption_secret = \"junk2\"\n[[tokens.keys]]\n\
         encryption_secret = \"{section_value}\"\n"
    );
    assert_eq!(key_of(&array_table), None);

    // A line of a multi-line string is not a line of the document.
    for delimiter in ["\"\"\"", "'''"] {
        let in_string = format!(
            "encryption_secret = \"junk\"\nencryption_secret = \"junk2\"\n\
             note = {delimiter}\nencryption_secret = \"{section_value}\"\n{delimiter}\n"
        );
        assert!(in_string.parse::<toml::Table>().is_err());
        assert_eq!(key_of(&in_string), None, "{delimiter}");
        // ... and the document goes on after the string closes.
        let after_string = format!("{in_string}encryption_secret = \"{top}\"\n");
        assert_eq!(key_of(&after_string), Some(top.clone()), "{delimiter}");
        // A string closed on the line it opens does not hide the next line.
        let inline = format!(
            "encryption_secret = \"junk\"\nnote = {delimiter}x{delimiter}\n\
             encryption_secret = \"{top}\"\n"
        );
        assert_eq!(key_of(&inline), Some(top.clone()), "{delimiter}");
    }

    // The copies: the section value is never written as the key-only copy, and
    // a parsed copy whose top-level value is no key is kept whole and reported.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("config.toml.backup"), &parsed).unwrap();
    std::fs::write(d.join("config.toml.backup.20261001T000000Z"), &after_table).unwrap();
    for _ in 0..2 {
        let report = protect_clear_copies(d, None);
        assert_eq!(report.failures.len(), 2, "{report:?}");
        assert_eq!(report.reduced, 0, "{report:?}");
        assert_eq!(
            std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
            parsed
        );
        assert_eq!(
            std::fs::read_to_string(d.join("config.toml.backup.20261001T000000Z")).unwrap(),
            after_table
        );
    }

    // No key in the stores: a section value never stands in for the key.
    let empty = tempfile::tempdir().unwrap();
    std::fs::write(empty.path().join("config.toml"), &parsed).unwrap();
    assert_eq!(sealing(empty.path(), true), Sealing::Absent);
    let with_section_key = tempfile::tempdir().unwrap();
    std::fs::write(with_section_key.path().join("config.toml"), &only_section).unwrap();
    assert_eq!(sealing(with_section_key.path(), true), Sealing::Absent);
    std::fs::write(with_section_key.path().join("config.toml"), &both).unwrap();
    assert_eq!(sealing(with_section_key.path(), true), Sealing::Key(top));
}

/// With no key anywhere, an `encryption_secret` that holds no key (under a table,
/// empty, commented out) does not keep a copy whole in clear for good: the copy
/// is dropped. Only a top-level, non-blank string that is no key is kept and
/// reported (it may be the only trace of one).
#[test]
fn only_a_top_level_non_blank_string_keeps_a_copy_without_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let k = crypto::generate_secret();
    // LEAKY ends with `[tokens]`: the line below is a value of that table.
    let droppable = [
        (
            "config.toml.backup",
            format!("{LEAKY}encryption_secret = \"{k}\"\n"),
        ),
        (
            "config.toml.backup.20261001T000000Z",
            format!("encryption_secret = \"\"\n{LEAKY}"),
        ),
        (
            "config.toml.backup.20261002T000000Z",
            format!("# encryption_secret = \"{k}\"\n{LEAKY}"),
        ),
        (
            "config.toml.backup.20261003T000000Z",
            format!("encryption_secret = \"  \"\n{LEAKY}"),
        ),
        // Does not parse, and the key line is below its first table.
        (
            "config.toml.backup.20261004T000000Z",
            format!("{LEAKY}encryption_secret = \"{k}\"\n[broken\n"),
        ),
        // A number can never be a key.
        (
            "config.toml.backup.20261006T000000Z",
            format!("encryption_secret = 12345\n{LEAKY}"),
        ),
        (
            "config.toml.corrupt.20261005T000000Z",
            format!("note = '''\nencryption_secret = \"{k}\"\n'''\n{LEAKY}"),
        ),
    ];
    for (name, text) in &droppable {
        std::fs::write(d.join(name), text).unwrap();
    }
    let report = protect_clear_copies(d, None);
    assert_eq!(
        (report.reduced, report.deferred, report.failures.len()),
        (droppable.len(), 0, 0),
        "{report:?}"
    );
    assert!(files_holding(d, NEEDLES, &[]).is_empty());
    assert!(names(d).is_empty(), "{:?}", names(d));
    assert_eq!(protect_clear_copies(d, None), Protected::default());

    // A top-level value that is no key stays, whatever else surrounds it.
    let kept = [
        (
            "config.toml.backup",
            format!("encryption_secret = \"junk\"\n{LEAKY}"),
        ),
        (
            "config.toml.backup.20261002T000000Z",
            format!("encryption_secret = \"junk\"\nencryption_secret = \"\"\n{LEAKY}[broken\n"),
        ),
    ];
    for (name, text) in &kept {
        std::fs::write(d.join(name), text).unwrap();
    }
    for _ in 0..2 {
        let report = protect_clear_copies(d, None);
        assert_eq!(
            (report.reduced, report.deferred, report.failures.len()),
            (0, 0, kept.len()),
            "{report:?}"
        );
        for (name, text) in &kept {
            assert_eq!(&std::fs::read_to_string(d.join(name)).unwrap(), text);
        }
    }
}

/// A retired-key file holding a value that is no key is deferred (sealed whole
/// once a key exists), but one holding an unreadable `encryption_secret` is
/// reported in `failures`, never deferred or dropped: with no key anywhere, and
/// through `protect_copies` when the stores prove it.
#[test]
fn an_unreadable_key_line_in_a_retired_key_file_is_a_failure_not_a_deferral() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let retired = |ts: &str| format!("config.toml.retired-key.{ts}");
    let junk = retired("20261001T000000Z");
    let bare = retired("20261002T000000Z");
    let empty = retired("20261003T000000Z");
    std::fs::write(d.join(&junk), "encryption_secret = \"junk\"\n").unwrap();
    std::fs::write(d.join(&bare), "an-old-value\n").unwrap();
    std::fs::write(d.join(&empty), "encryption_secret = \"\"\n").unwrap();

    for report in [
        protect_clear_copies(d, None),
        protect_copies(d, &Sealing::Absent),
    ] {
        assert_eq!(
            (report.deferred, report.failures.len(), report.reduced),
            (2, 1, 0),
            "{report:?}"
        );
        assert!(report.failures[0].starts_with(&junk), "{report:?}");
    }
    assert_eq!(
        std::fs::read_to_string(d.join(&junk)).unwrap(),
        "encryption_secret = \"junk\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(d.join(&bare)).unwrap(),
        "an-old-value\n"
    );
    assert_eq!(
        std::fs::read_to_string(d.join(&empty)).unwrap(),
        "encryption_secret = \"\"\n"
    );

    // Once a key exists, the deferred ones are sealed whole.
    let key = crypto::generate_secret();
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!((report.sealed, report.deferred), (3, 0), "{report:?}");
    assert!(names(d).iter().all(|n| n.ends_with(".enc")));
}

/// A copy holding two different keys (a duplicate key makes it invalid TOML)
/// is never cut down to one line: the other may be the only copy of a live key.
/// The same key twice is one key.
#[test]
fn a_copy_holding_two_different_keys_is_never_cut_down() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let (k1, k2) = (crypto::generate_secret(), crypto::generate_secret());
    let two = format!("encryption_secret = \"{k1}\"\nencryption_secret = \"{k2}\"\n{LEAKY}");
    assert!(two.parse::<toml::Table>().is_err());
    // The first key stays the key for whoever needs one.
    assert_eq!(key_of(&two), Some(k1.clone()));

    std::fs::write(d.join("config.toml.backup"), &two).unwrap();
    for key_hex in [None, Some(key.as_str())] {
        // With a key, make the seal fail so that the cut-down is attempted.
        if key_hex.is_some() {
            std::fs::create_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();
        }
        let report = protect_clear_copies(d, key_hex);
        assert_eq!(
            (report.failures.len(), report.reduced, report.sealed),
            (1, 0, 0),
            "{report:?}"
        );
        assert!(report.failures[0].contains("different encryption_secret keys"));
        assert_eq!(
            std::fs::read_to_string(d.join("config.toml.backup")).unwrap(),
            two
        );
    }
    std::fs::remove_dir(d.join(".config.toml.backup.enc.tmp")).unwrap();

    // Sealed whole when the seal works: both keys are in the ciphertext.
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!((report.sealed, report.failures.len()), (1, 0), "{report:?}");
    assert_eq!(
        open_sealed(&d.join("config.toml.backup.enc"), &[key]).unwrap(),
        two
    );

    // The same key twice (or in another spelling) is one key.
    let same = format!("encryption_secret = \"{k1}\"\nencryption_secret = \"{k1}\"\n{LEAKY}");
    assert!(same.parse::<toml::Table>().is_err());
    std::fs::write(d.join("config.toml.backup.20261001T000000Z"), &same).unwrap();
    let report = protect_clear_copies(d, None);
    assert_eq!(
        (report.reduced, report.failures.len()),
        (1, 0),
        "{report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("config.toml.backup.20261001T000000Z")).unwrap(),
        format!("encryption_secret = \"{k1}\"\n")
    );
}

/// A copy whose seal fails and that holds no key line is removed (nothing a later
/// start needs); a `corrupt` copy is never touched on a failed seal: a config.toml
/// that could never be read may hold credentials that reached no encrypted store.
#[test]
fn a_failed_seal_drops_a_keyless_copy_but_never_touches_a_corrupt_one() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let corrupt = "config.toml.corrupt.20261001T000000Z";
    let with_key = format!(
        "encryption_secret = \"{}\"\n{LEAKY}",
        crypto::generate_secret()
    );
    std::fs::write(d.join("config.toml.backup"), LEAKY).unwrap();
    std::fs::write(d.join(corrupt), &with_key).unwrap();
    let corrupt_keyless = "config.toml.corrupt.20261002T000000Z";
    std::fs::write(d.join(corrupt_keyless), LEAKY).unwrap();
    for name in ["config.toml.backup", corrupt, corrupt_keyless] {
        std::fs::create_dir(d.join(format!(".{name}.enc.tmp"))).unwrap();
    }

    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(
        (report.reduced, report.sealed, report.deferred),
        (1, 0, 0),
        "{report:?}"
    );
    assert_eq!(report.failures.len(), 2, "{report:?}");
    assert!(
        report.failures.iter().all(|f| f.contains("left as it is")),
        "{report:?}"
    );
    assert!(!d.join("config.toml.backup").exists());
    assert_eq!(std::fs::read_to_string(d.join(corrupt)).unwrap(), with_key);
    assert_eq!(
        std::fs::read_to_string(d.join(corrupt_keyless)).unwrap(),
        LEAKY
    );

    // Once the disk allows it the corrupt copies are sealed whole.
    for name in [corrupt, corrupt_keyless] {
        std::fs::remove_dir(d.join(format!(".{name}.enc.tmp"))).unwrap();
    }
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!((report.sealed, report.failures.len()), (2, 0), "{report:?}");
    assert!(files_holding(d, NEEDLES, &["config.toml"]).is_empty());
}

/// A FIFO named like a copy is refused, not read: without the check `fs::read`
/// would block the start pass for good.
#[cfg(unix)]
#[test]
fn a_fifo_copy_is_refused_and_does_not_block_the_pass() {
    use std::os::unix::ffi::OsStrExt;

    let (dir, key) = dir_with_key();
    let d = dir.path().to_path_buf();
    let fifo = d.join("config.toml.backup");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `path` is a NUL-terminated string that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

    let (done, waiting) = std::sync::mpsc::channel();
    let worker_dir = d.clone();
    std::thread::spawn(move || {
        let with_key = protect_clear_copies(&worker_dir, Some(&key));
        let without = protect_clear_copies(&worker_dir, None);
        let _ = done.send((with_key, without));
    });
    // A blocked pass fails the test instead of hanging it.
    let (with_key, without) = waiting
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the pass blocked on a FIFO");
    for report in [with_key, without] {
        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert!(
            report.failures[0].contains("not a regular file"),
            "{report:?}"
        );
    }
    assert!(std::fs::symlink_metadata(&fifo).is_ok());
}

/// A symbolic link is never read through, renamed over or removed (its target
/// would keep the content in clear), and a file with another hard link is left
/// alone: both are failures, with or without a key.
#[cfg(unix)]
#[test]
fn a_linked_copy_is_refused_and_reported() {
    let (dir, key) = dir_with_key();
    let d = dir.path();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("target");
    std::fs::write(&target, LEAKY).unwrap();
    std::os::unix::fs::symlink(&target, d.join("config.toml.backup")).unwrap();
    std::fs::write(d.join("config.toml.corrupt.20261001T000000Z"), LEAKY).unwrap();
    std::fs::hard_link(
        d.join("config.toml.corrupt.20261001T000000Z"),
        outside.path().join("other-name"),
    )
    .unwrap();

    for key_hex in [Some(key.as_str()), None] {
        let report = protect_clear_copies(d, key_hex);
        assert_eq!(report.failures.len(), 2, "{report:?}");
        assert_eq!((report.sealed, report.reduced), (0, 0), "{report:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), LEAKY);
        assert!(d
            .join("config.toml.backup")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(d.join("config.toml.corrupt.20261001T000000Z")).unwrap(),
            LEAKY
        );
        assert!(!d.join("config.toml.backup.enc").exists());
        assert!(!d.join("config.toml.corrupt.20261001T000000Z.enc").exists());
    }
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
        format!("encryption_secret = \"{k}\"\n{BROKEN}"),
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

/// A top-level key line the line scanner cannot read is never dropped (with no
/// key anywhere nothing else holds it): kept as it is and reported. One input
/// per way a line can be damaged.
#[test]
fn a_damaged_key_line_keeps_the_copy_whole_and_reported() {
    let k = crypto::generate_secret();
    let inputs = [
        // (a) missing closing quote
        format!(
            "encryption_secret = \"{k}\nlanguage = \"fr\"\n[tokens]\nanthropic = \"sk-clear\"\n"
        ),
        // (b) trailing text
        format!("encryption_secret = \"{k}\" x\n{LEAKY}"),
        // (c) a multi-line string, in a text that does not parse
        format!("encryption_secret = \"\"\"\n{k}\"\"\"\nbroken\n{LEAKY}"),
        format!("encryption_secret = '''\n{k}'''\nbroken\n{LEAKY}"),
        // (d) an intact key line below a line that starts with `[` in an array
        // left open: that line is an element, not a table
        format!("a = [\n  [1, 2],\nencryption_secret = \"{k}\"\n{LEAKY}"),
        // (e) a key line swallowed by a multi-line string left open above it
        format!("a = \"\"\"\nencryption_secret = \"{k}\"\n{LEAKY}"),
        format!("a = '''\nencryption_secret = \"{k}\"\n{LEAKY}"),
        // a readable key beside a value that cannot be read
        format!("encryption_secret = \"{k}\"\nencryption_secret = \"junk\"\n{LEAKY}"),
        format!("encryption_secret = \"{k}\"\nencryption_secret = \"{k}\" x\n{LEAKY}"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for (i, text) in inputs.iter().enumerate() {
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert!(key_lines(text).unreadable, "{text}");
        std::fs::write(
            d.join(format!("config.toml.backup.2026100{i}T000000Z")),
            text,
        )
        .unwrap();
    }
    for _ in 0..2 {
        let report = protect_clear_copies(d, None);
        assert_eq!(
            (report.reduced, report.deferred, report.failures.len()),
            (0, 0, inputs.len()),
            "{report:?}"
        );
        for (i, text) in inputs.iter().enumerate() {
            let name = format!("config.toml.backup.2026100{i}T000000Z");
            assert_eq!(&std::fs::read_to_string(d.join(name)).unwrap(), text);
        }
    }

    // The same through the stores' proof, and sealed whole once a key exists.
    let report = protect_copies(d, &Sealing::Absent);
    assert_eq!(report.failures.len(), inputs.len(), "{report:?}");
    let key = crypto::generate_secret();
    let report = protect_clear_copies(d, Some(&key));
    assert_eq!(
        (report.sealed, report.failures.len()),
        (inputs.len(), 0),
        "{report:?}"
    );
    assert!(names(d).iter().all(|n| n.ends_with(".enc")));
}

/// A mention proven harmless does not keep a credential copy in clear, in a text
/// that does not parse either: a comment, a value under a table the scan reached
/// with nothing open (a byte-order mark before the header included), a blank or
/// non-string value, an array element that is not a key line.
#[test]
fn a_mention_proven_harmless_is_dropped_even_in_a_text_that_does_not_parse() {
    let k = crypto::generate_secret();
    let droppable = [
        format!("{LEAKY}encryption_secret = \"{k}\"\n[broken\n"),
        format!("# encryption_secret = \"{k}\"\n{LEAKY}[broken\n"),
        format!("language = \"fr\" # encryption_secret = \"{k}\"\n{LEAKY}[broken\n"),
        format!("encryption_secret = \"\"\nbroken\n{LEAKY}"),
        format!("encryption_secret = 7\n[broken\n{LEAKY}"),
        // A byte-order mark before a table header: the table is still a table.
        format!("\u{FEFF}[tokens]\nencryption_secret = \"{k}\"\n{LEAKY}[broken\n"),
        // An array closed before the key's table: the `[` below it is a header.
        format!("a = [\n  [1, 2],\n]\n[tokens]\nencryption_secret = \"{k}\"\n[broken\n"),
        // A string closed before it.
        format!("a = \"\"\"\nx\n\"\"\"\n[tokens]\nencryption_secret = \"{k}\"\n[broken\n"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for (i, text) in droppable.iter().enumerate() {
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert_eq!(key_of(text), None, "{text}");
        assert!(!config::salvage_key_scan(text).unreadable, "{text}");
        std::fs::write(
            d.join(format!("config.toml.backup.2026100{i}T000000Z")),
            text,
        )
        .unwrap();
    }
    let report = protect_clear_copies(d, None);
    assert_eq!(
        (report.reduced, report.deferred, report.failures.len()),
        (droppable.len(), 0, 0),
        "{report:?}"
    );
    assert!(names(d).is_empty(), "{:?}", names(d));

    // A byte-order mark does not hide a key line on the first line either.
    let bom_key = format!("\u{FEFF}encryption_secret = \"{k}\"\n[broken\n");
    assert_eq!(key_of(&bom_key), Some(k));
}

/// The key line written with a quoted or escaped key is the key line: TOML decodes
/// it to the same name, so a copy is cut down to it (and a config.toml set aside
/// keeps it), never dropped; one that cannot be read is kept whole and reported.
#[test]
fn a_quoted_or_escaped_key_name_is_still_the_key_line() {
    let k = crypto::generate_secret();
    let spellings = [
        "\"encryption_secret\"",
        "'encryption_secret'",
        "\"encryption\\u005fsecret\"",
        "\"\\u0065ncryption_secret\"",
        "  \"encryption_secret\"  ",
    ];
    for (i, name) in spellings.iter().enumerate() {
        let text = format!("{name} = \"{k}\"\nbroken\n{LEAKY}");
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert_eq!(key_of(&text), Some(k.clone()), "{text}");
        assert_eq!(config::salvage_key_line(&text), Some(k.clone()), "{text}");

        let dir = tempfile::tempdir().unwrap();
        let copy = format!("config.toml.backup.2026100{i}T000000Z");
        std::fs::write(dir.path().join(&copy), &text).unwrap();
        let report = protect_clear_copies(dir.path(), None);
        assert_eq!(
            (report.reduced, report.failures.len()),
            (1, 0),
            "{report:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&copy)).unwrap(),
            format!("encryption_secret = \"{k}\"\n"),
            "{text}"
        );
        assert!(files_holding(dir.path(), NEEDLES, &[]).is_empty());

        let bare = tempfile::tempdir().unwrap();
        let Aside::KeyOnly(kept) = set_aside_content(bare.path(), text.as_bytes(), None).unwrap()
        else {
            panic!("the key line is kept: {text}")
        };
        assert_eq!(
            std::fs::read_to_string(bare.path().join(kept)).unwrap(),
            format!("encryption_secret = \"{k}\"\n")
        );
    }

    // A quoted key line that cannot be read, or whose value is no key, is never
    // dropped: kept whole and reported.
    let damaged = [
        format!("\"encryption_secret\" = \"{k}\nbroken\n{LEAKY}"),
        format!("\"encryption_secret\" = \"{k}\" x\n{LEAKY}"),
        format!("\"encryption\\u005fsecret\" = \"{k}\" x\n{LEAKY}"),
        format!("\"encryption\\u005fsecret\" = \"{k}\n{LEAKY}"),
        format!("\"encryption\\u005fsecret\" = \"junk\"\nbroken\n{LEAKY}"),
        format!("'encryption_secret' = '''\n{k}'''\nbroken\n{LEAKY}"),
        // A quoted key that cannot be told from the key when it does not parse.
        "\"enc\\u0072yption_secret\" = [\nbroken\n".to_string(),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (i, text) in damaged.iter().enumerate() {
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert!(key_lines(text).unreadable, "{text}");
        std::fs::write(
            dir.path()
                .join(format!("config.toml.backup.2026100{i}T000000Z")),
            text,
        )
        .unwrap();
        let bare = tempfile::tempdir().unwrap();
        assert!(
            matches!(
                set_aside_content(bare.path(), text.as_bytes(), None).unwrap(),
                Aside::KeptAsIs(_)
            ),
            "{text}"
        );
    }
    for _ in 0..2 {
        let report = protect_clear_copies(dir.path(), None);
        assert_eq!(
            (report.reduced, report.failures.len()),
            (0, damaged.len()),
            "{report:?}"
        );
        for (i, text) in damaged.iter().enumerate() {
            let copy = format!("config.toml.backup.2026100{i}T000000Z");
            assert_eq!(
                &std::fs::read_to_string(dir.path().join(copy)).unwrap(),
                text
            );
        }
    }

    // A plain line that does not parse and is not the key stays harmless.
    for harmless in ["language = \"fr\nbroken\n[x]\n", "a = [\nb = 1\n"] {
        assert!(!config::salvage_key_scan(harmless).unreadable, "{harmless}");
    }
}

/// A quoted or escaped key line inside a string or an array that is still open is
/// not proven harmless either: whatever its spelling, it may be the live key, so
/// the copy is kept whole (bytes unchanged) and reported, and a config.toml set
/// aside keeps its whole content. A quoted array element is still harmless.
#[test]
fn a_quoted_or_escaped_key_line_inside_an_open_region_keeps_the_copy_whole() {
    let k = crypto::generate_secret();
    let hidden = [
        // An array that never closes.
        format!("x = [\n\"encryption_secret\" = \"{k}\"\n"),
        format!("x = [\n\"encryption\\u005fsecret\" = \"{k}\"\n"),
        format!("x = [\n  \"\\u0065ncryption_secret\" = \"{k}\"\n"),
        format!("x = [\n'encryption_secret' = '{k}'\n"),
        // A multi-line string that never closes, or does and leaves the text broken.
        format!("x = \"\"\"\n\"encryption_secret\" = \"{k}\"\n"),
        format!("x = \"\"\"\n\"enc\\u0072yption_secret\" = \"{k}\"\n\"\"\"\nbroken\n"),
        format!("x = '''\n\"encryption\\u005fsecret\" = \"{k}\"\n"),
        // An inline table over several lines.
        format!("x = {{\n\"encryption\\u005fsecret\" = \"{k}\",\n"),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (i, text) in hidden.iter().enumerate() {
        let text = format!("pat = \"pat-clear\"\ncookie = \"cookie-clear\"\n{text}");
        assert!(text.parse::<toml::Table>().is_err(), "{text}");
        assert!(config::salvage_key_scan(&text).unreadable, "{text}");
        assert!(key_lines(&text).unreadable, "{text}");
        std::fs::write(
            dir.path()
                .join(format!("config.toml.backup.2026100{i}T000000Z")),
            &text,
        )
        .unwrap();
        let bare = tempfile::tempdir().unwrap();
        assert!(
            matches!(
                set_aside_content(bare.path(), text.as_bytes(), None).unwrap(),
                Aside::KeptAsIs(_)
            ),
            "{text}"
        );
    }
    for _ in 0..2 {
        let report = protect_clear_copies(dir.path(), None);
        assert_eq!(
            (report.reduced, report.failures.len()),
            (0, hidden.len()),
            "{report:?}"
        );
        for (i, text) in hidden.iter().enumerate() {
            let copy = format!("config.toml.backup.2026100{i}T000000Z");
            let kept = std::fs::read_to_string(dir.path().join(copy)).unwrap();
            assert!(kept.ends_with(text.as_str()), "{kept}");
        }
    }

    // Quoted array elements and plain lines in an open region are harmless.
    for harmless in [
        "x = [\n\"a\",\n\"b\"\n",
        "x = \"\"\"\nplain prose\n",
        "x = [\n{ a = 1 },\n",
    ] {
        assert!(!config::salvage_key_scan(harmless).unreadable, "{harmless}");
    }
}

/// An unreadable config.toml set aside with no key: several different keys, or a
/// key line that cannot be read, keep the whole content (owner-only) and say so,
/// never one key line or nothing; one readable key is still the only thing kept.
#[test]
fn an_unreadable_config_with_several_keys_or_a_damaged_key_line_is_kept_whole() {
    let (k1, k2) = (crypto::generate_secret(), crypto::generate_secret());
    let kept = [
        format!("encryption_secret = \"{k1}\"\nencryption_secret = \"{k2}\"\n{LEAKY}"),
        format!("encryption_secret = \"garbled\"\nx = [broken\n{LEAKY}"),
        format!("encryption_secret = \"{k1}\nlanguage = \"fr\"\n{LEAKY}"),
        format!("encryption_secret = \"{k1}\"\nencryption_secret = \"junk\"\n{LEAKY}"),
    ];
    for text in &kept {
        let bare = tempfile::tempdir().unwrap();
        let Aside::KeptAsIs(name) = set_aside_content(bare.path(), text.as_bytes(), None).unwrap()
        else {
            panic!("kept whole: {text}")
        };
        assert!(name.starts_with(CORRUPT_PREFIX), "{name}");
        assert_eq!(
            &std::fs::read_to_string(bare.path().join(&name)).unwrap(),
            text
        );
        assert!(Aside::KeptAsIs(name.clone()).describe().contains(&name));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bare.path().join(&name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No other file, no temp file left behind.
        assert_eq!(names(bare.path()), vec![name.clone()]);

        // The next start reports it, and seals it once a key exists.
        let report = protect_clear_copies(bare.path(), None);
        assert_eq!(
            (report.reduced, report.failures.len()),
            (0, 1),
            "{report:?}"
        );
        assert_eq!(
            &std::fs::read_to_string(bare.path().join(&name)).unwrap(),
            text
        );
        let key = crypto::generate_secret();
        let report = protect_clear_copies(bare.path(), Some(&key));
        assert_eq!(report.sealed, 1, "{report:?}");
        assert_eq!(
            open_sealed(&bare.path().join(format!("{name}.enc")), &[key]).unwrap(),
            *text
        );
    }

    // One readable key, even written twice: only the key line is kept.
    let same = format!("encryption_secret = \"{k1}\"\nencryption_secret = \"{k1}\"\n{LEAKY}");
    let bare = tempfile::tempdir().unwrap();
    let Aside::KeyOnly(name) = set_aside_content(bare.path(), same.as_bytes(), None).unwrap()
    else {
        panic!("one key is kept alone")
    };
    assert_eq!(
        std::fs::read_to_string(bare.path().join(name)).unwrap(),
        format!("encryption_secret = \"{k1}\"\n")
    );
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
    // A written config.toml keeps its scalars, the key included, above any table.
    let text = format!("encryption_secret = \"{k}\"\n{LEAKY}[broken");
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
/// for lack of a key: kept (and reported, as it holds an unreadable
/// `encryption_secret`) until one exists, then sealed whole. Only a file proven
/// empty goes.
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
    assert_eq!(
        (report.deferred, report.reduced, report.failures.len()),
        (0, 1, 1),
        "{report:?}"
    );
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
