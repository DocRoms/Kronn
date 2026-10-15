//! The backend binary takes no arguments; `--version` and `--help` answer and
//! exit before anything is opened. Any other argument is refused: an ignored
//! flag used to boot a full backend against the default data dir.

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Run,
    PrintVersion,
    PrintHelp,
    Refuse(String),
}

pub fn cli_action(args: &[String]) -> CliAction {
    match args {
        [] => CliAction::Run,
        [one] if one == "--version" || one == "-V" => CliAction::PrintVersion,
        [one] if one == "--help" || one == "-h" => CliAction::PrintHelp,
        _ => CliAction::Refuse(format!(
            "unexpected argument(s): {}. The backend takes none; configure it through \
             config.toml or KRONN_DATA_DIR, KRONN_HOST, KRONN_PORT.",
            args.join(" ")
        )),
    }
}

pub const HELP: &str = "kronn — the Kronn backend.\n\nUsage: kronn [--version | --help]\n\n\
Environment: KRONN_DATA_DIR (data directory), KRONN_HOST (bind address), \
KRONN_PORT (listening port for this process).";

#[cfg(test)]
mod tests {
    use super::{cli_action, CliAction};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn only_no_argument_runs_the_backend() {
        assert_eq!(cli_action(&args(&[])), CliAction::Run);
        assert_eq!(cli_action(&args(&["--version"])), CliAction::PrintVersion);
        assert_eq!(cli_action(&args(&["-V"])), CliAction::PrintVersion);
        assert_eq!(cli_action(&args(&["--help"])), CliAction::PrintHelp);
        for refused in [&["--port", "4000"][..], &["serve"], &["--version", "x"]] {
            assert!(
                matches!(cli_action(&args(refused)), CliAction::Refuse(_)),
                "{refused:?} must not boot a backend"
            );
        }
    }
}
