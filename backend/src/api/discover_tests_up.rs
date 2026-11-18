use super::*;

#[test]
fn the_glab_listing_gets_no_backend_secret_and_no_token() {
    use crate::core::child_env::probe;
    let command = probe::with_secret_parent("/usr/bin", "/home/u", || {
        gitlab_cli_command("gitlab.example.com")
    });
    probe::assert_built_without_secrets(command.as_std(), "/usr/bin", &[]);
}
