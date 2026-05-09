use std::process::Command;

fn watchr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_watchr"))
}

#[test]
fn help_includes_linux_watch_options() {
    let output = watchr().arg("--help").output().expect("run watchr --help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help output is utf-8");
    for option in [
        "--beep",
        "--differences",
        "--errexit",
        "--chgexit",
        "--precise",
        "--equexit",
        "--exec",
        "--count",
        "--duration",
        "--until",
    ] {
        assert!(stdout.contains(option), "help output missing {option}");
    }
}

#[test]
fn version_reports_package_version() {
    let output = watchr()
        .arg("--version")
        .output()
        .expect("run watchr --version");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("version output is utf-8");
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn mutually_exclusive_limits_are_rejected() {
    let output = watchr()
        .args(["--count", "1", "--duration", "1s", "echo"])
        .output()
        .expect("run watchr with conflicting limit options");

    assert!(!output.status.success());
}
