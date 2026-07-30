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
        "--color",
        "--no-color",
        "--differences",
        "--errexit",
        "--follow",
        "--chgexit",
        "--precise",
        "--equexit",
        "--no-rerun",
        "--shotsdir",
        "--no-title",
        "--no-wrap",
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
    for flag in ["-v", "--version"] {
        let output = watchr().arg(flag).output().expect("run watchr version");

        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("version output is utf-8");
        assert_eq!(
            stdout.trim(),
            format!("watchr {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn mutually_exclusive_limits_are_rejected() {
    let output = watchr()
        .args(["--count", "1", "--duration", "1s", "echo"])
        .output()
        .expect("run watchr with conflicting limit options");

    assert!(!output.status.success());
}

#[test]
fn follow_rejects_output_tracking_options() {
    for tracking_option in ["--differences", "--chgexit", "--equexit=2"] {
        let output = watchr()
            .args(["--follow", tracking_option, "echo"])
            .output()
            .expect("run watchr with conflicting follow option");

        assert!(!output.status.success(), "{tracking_option} was accepted");
    }
}

#[test]
fn invalid_limit_and_interval_values_are_rejected() {
    for args in [
        ["--count", "0", "echo"].as_slice(),
        ["--duration", "0s", "echo"].as_slice(),
        ["--interval", "NaN", "echo"].as_slice(),
    ] {
        let output = watchr()
            .args(args)
            .output()
            .expect("run watchr with an invalid value");
        assert!(!output.status.success(), "{args:?} was accepted");
    }
}
