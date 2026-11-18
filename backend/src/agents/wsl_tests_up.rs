
use super::*;

#[cfg(unix)]
// asserts POSIX host paths (/usr/bin/claude, /work); the Windows build wraps through wsl.exe instead
#[test]
fn unix_hosts_never_wrap_through_wsl() {
    let (program, args, cwd) = crate::agents::runner::platform_agent_invocation(
        "/usr/bin/claude".into(),
        vec!["--add-dir".into(), r"C:\other".into()],
        true,
        Path::new("/work"),
    );
    assert_eq!(program, "/usr/bin/claude");
    assert_eq!(args, vec!["--add-dir", r"C:\other"]);
    assert_eq!(cwd, Path::new("/work"));
}
