use std::{fmt::Debug, io, path::PathBuf, process::ExitCode, time::Duration};

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use clap::{crate_authors, ArgAction, Parser};
use watch_rs::{ExecutionLimit, WatchOptions};

const MIN_INTERVAL: f64 = 0.1;
const MAX_INTERVAL: f64 = 60.0 * 60.0 * 24.0 * 31.0;
const SUCCESSIVE_DIFFERENCES: &str = "successive";

#[derive(Parser, Debug)]
#[command(
    name = "watchr",
    version,
    disable_version_flag = true,
    author = crate_authors!(),
    about,
    long_about = None,
    trailing_var_arg = true
)]
#[command(help_template(
    "\
{before-help}{name} {version}
Author: {author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
"
))]
struct Args {
    /// Seconds to wait between updates (0.1 to 2678400)
    #[arg(
        name = "interval",
        short = 'n',
        long,
        value_name = "secs",
        default_value = "2.0",
        env = "WATCH_INTERVAL",
        allow_hyphen_values = true,
        value_parser = parse_interval
    )]
    interval: f64,

    /// Beep if the command has a non-zero exit
    #[arg(short = 'b', long)]
    beep: bool,

    /// Interpret ANSI color and style sequences
    #[arg(short = 'c', long)]
    color: bool,

    /// Do not interpret ANSI color and style sequences
    #[arg(short = 'C', long = "no-color")]
    no_color: bool,

    /// Highlight changes between updates; an attached value enables permanent mode
    #[arg(
        short = 'd',
        long,
        value_name = "permanent",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = SUCCESSIVE_DIFFERENCES
    )]
    differences: Option<String>,

    /// Enable permanent differences with the canonical `-d1` form
    #[arg(short = '1', hide = true, requires = "differences")]
    differences_short_permanent: bool,

    /// Highlight all changes since the first update
    #[arg(long = "differences-permanent")]
    differences_permanent: bool,

    /// Exit if the command has a non-zero exit
    #[arg(short = 'e', long)]
    errexit: bool,

    /// Follow output without clearing the screen
    #[arg(
        short = 'f',
        long,
        conflicts_with_all = [
            "differences",
            "differences_permanent",
            "chgexit",
            "equexit"
        ]
    )]
    follow: bool,

    /// Exit when the visible output changes
    #[arg(short = 'g', long)]
    chgexit: bool,

    /// Include command running time in the update interval
    #[arg(short = 'p', long)]
    precise: bool,

    /// Exit when visible output is unchanged for this many cycles
    #[arg(
        short = 'q',
        long,
        value_name = "cycles",
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    equexit: Option<u32>,

    /// Do not rerun the command when the terminal is resized
    #[arg(short = 'r', long = "no-rerun")]
    no_rerun: bool,

    /// Directory in which screenshots are saved
    #[arg(short = 's', long = "shotsdir", value_name = "dir")]
    shots_dir: Option<PathBuf>,

    /// Turn off the header
    #[arg(short = 't', long = "no-title")]
    no_title: bool,

    /// Truncate long lines instead of wrapping
    #[arg(short = 'w', long = "no-wrap")]
    no_wrap: bool,

    /// Execute the command directly instead of through a shell
    #[arg(short = 'x', long)]
    exec: bool,

    /// Execute the command exactly N times
    #[arg(
        long = "count",
        value_name = "N",
        conflicts_with_all = ["duration", "until"],
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    count: Option<u64>,

    /// Execute for a duration such as 1h30m, 45s, or 2h
    #[arg(
        long = "duration",
        value_name = "duration",
        conflicts_with_all = ["count", "until"],
        value_parser = parse_duration
    )]
    duration: Option<Duration>,

    /// Execute until an ISO 8601 date and time
    #[arg(
        long = "until",
        value_name = "datetime",
        conflicts_with_all = ["count", "duration"],
        value_parser = parse_until_datetime
    )]
    until: Option<DateTime<Local>>,

    /// Display version information and exit
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: Option<bool>,

    /// Command and arguments to run
    #[arg(name = "command", required = true, num_args = 1.., allow_hyphen_values = true)]
    command: Vec<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code.clamp(0, u8::MAX as i32) as u8),
        Err(error) => {
            eprintln!("watchr: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<i32> {
    let args = Args::parse();
    let (command, command_args) = args.command.split_first().expect("command is required");

    let execution_limit = args
        .count
        .map(ExecutionLimit::Count)
        .or_else(|| args.duration.map(ExecutionLimit::Duration))
        .or_else(|| args.until.map(ExecutionLimit::Until));

    let differences_permanent = args.differences_permanent
        || args.differences_short_permanent
        || args
            .differences
            .as_deref()
            .is_some_and(|value| value != SUCCESSIVE_DIFFERENCES);

    watch_rs::watch_with_exit_code(WatchOptions {
        command: command.clone(),
        args: command_args.to_vec(),
        interval: args.interval,
        beep: args.beep,
        color: args.color && !args.no_color,
        no_color: args.no_color,
        differences: args.differences.is_some() || differences_permanent,
        differences_permanent,
        errexit: args.errexit,
        follow: args.follow,
        chgexit: args.chgexit,
        precise: args.precise,
        equexit: args.equexit,
        no_rerun: args.no_rerun,
        shots_dir: args.shots_dir,
        no_title: args.no_title,
        no_wrap: args.no_wrap,
        exec: args.exec,
        execution_limit,
    })
}

fn parse_interval(input: &str) -> Result<f64, String> {
    let normalized = input.trim().replace(',', ".");
    let interval = normalized
        .parse::<f64>()
        .map_err(|_| format!("invalid interval '{input}'"))?;

    if !interval.is_finite() {
        return Err("interval must be a finite number".to_string());
    }

    Ok(interval.clamp(MIN_INTERVAL, MAX_INTERVAL))
}

fn parse_until_datetime(input: &str) -> Result<DateTime<Local>, String> {
    if let Ok(datetime) = DateTime::parse_from_rfc3339(input) {
        return Ok(datetime.with_timezone(&Local));
    }

    parse_local_datetime(input, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| parse_local_datetime(input, "%Y-%m-%d %H:%M:%S"))
        .map_err(|error| {
            format!(
                "{error}; use ISO 8601 format, for example 2030-12-31T23:59:59 or an RFC 3339 offset"
            )
        })
}

fn parse_local_datetime(input: &str, format: &str) -> Result<DateTime<Local>, String> {
    let datetime =
        NaiveDateTime::parse_from_str(input, format).map_err(|error| error.to_string())?;
    Local
        .from_local_datetime(&datetime)
        .single()
        .ok_or_else(|| "datetime is ambiguous or does not exist in the local timezone".to_string())
}

fn parse_duration(input: &str) -> Result<Duration, String> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return Err("duration cannot be empty".to_string());
    }

    let mut total_seconds = 0_u64;
    let mut current_number = String::new();

    for character in input.chars() {
        if character.is_ascii_digit() {
            current_number.push(character);
            continue;
        }

        if current_number.is_empty() {
            return Err(format!("unexpected '{character}' in duration"));
        }

        let value = current_number
            .parse::<u64>()
            .map_err(|_| "invalid duration number".to_string())?;
        current_number.clear();

        let multiplier = match character {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 60 * 60 * 24,
            _ => return Err(format!("unknown duration unit '{character}'")),
        };
        let seconds = value
            .checked_mul(multiplier)
            .ok_or_else(|| "duration is too large".to_string())?;
        total_seconds = total_seconds
            .checked_add(seconds)
            .ok_or_else(|| "duration is too large".to_string())?;
    }

    if !current_number.is_empty() {
        let seconds = current_number
            .parse::<u64>()
            .map_err(|_| "invalid duration number".to_string())?;
        total_seconds = total_seconds
            .checked_add(seconds)
            .ok_or_else(|| "duration is too large".to_string())?;
    }

    if total_seconds == 0 {
        return Err("duration must be greater than zero".to_string());
    }

    Ok(Duration::from_secs(total_seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn interval_accepts_both_decimal_separators_and_clamps_bounds() {
        assert_eq!(parse_interval("1.5").unwrap(), 1.5);
        assert_eq!(parse_interval("1,5").unwrap(), 1.5);
        assert_eq!(parse_interval("0").unwrap(), MIN_INTERVAL);
        assert_eq!(parse_interval("999999999").unwrap(), MAX_INTERVAL);
    }

    #[test]
    fn interval_rejects_non_finite_values() {
        assert!(parse_interval("NaN").is_err());
        assert!(parse_interval("inf").is_err());
    }

    #[test]
    fn duration_parses_single_and_combined_units() {
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(
            parse_duration("1h30m45s").unwrap(),
            Duration::from_secs(5_445)
        );
        assert_eq!(parse_duration("1D").unwrap(), Duration::from_secs(86_400));
    }

    #[test]
    fn duration_rejects_invalid_and_overflowing_values() {
        for invalid in ["", "abc", "0", "1.5s", "s1"] {
            assert!(parse_duration(invalid).is_err(), "{invalid} was accepted");
        }
        assert!(parse_duration(&format!("{}d", u64::MAX)).is_err());
    }

    #[test]
    fn until_accepts_rfc3339_and_local_formats() {
        let parsed = parse_until_datetime("2026-05-09T08:44:07Z").unwrap();
        assert_eq!(
            parsed.with_timezone(&chrono::Utc).to_rfc3339(),
            "2026-05-09T08:44:07+00:00"
        );
        assert!(parse_until_datetime("2030-12-31 23:59:59").is_ok());
        assert!(parse_until_datetime("not-a-date").is_err());
    }

    #[test]
    fn canonical_difference_forms_are_accepted() {
        let successive = Args::try_parse_from(["watchr", "-d", "echo"]).unwrap();
        assert_eq!(
            successive.differences.as_deref(),
            Some(SUCCESSIVE_DIFFERENCES)
        );

        let short_permanent = Args::try_parse_from(["watchr", "-d1", "echo"]).unwrap();
        assert_eq!(
            short_permanent.differences.as_deref(),
            Some(SUCCESSIVE_DIFFERENCES)
        );
        assert!(short_permanent.differences_short_permanent);

        let long_permanent =
            Args::try_parse_from(["watchr", "--differences=permanent", "echo"]).unwrap();
        assert_eq!(long_permanent.differences.as_deref(), Some("permanent"));
    }

    #[test]
    fn command_options_are_trailing_arguments() {
        let args = Args::try_parse_from(["watchr", "--exec", "echo", "-n", "hello"]).unwrap();
        assert_eq!(args.interval, 2.0);
        assert_eq!(args.command, ["echo", "-n", "hello"]);
    }

    #[test]
    fn follow_rejects_screen_tracking_modes() {
        let error = Args::try_parse_from(["watchr", "--follow", "--chgexit", "echo"])
            .expect_err("conflicting options should fail");
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn zero_count_is_rejected() {
        assert!(Args::try_parse_from(["watchr", "--count", "0", "echo"]).is_err());
    }
}
