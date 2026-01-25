use std::fmt::Debug;
use std::io::Result;

use clap::{crate_authors, Parser};
use watch_rs::{ExecutionLimit, WatchOptions};

#[derive(Parser, Debug)]
#[command(version, author = crate_authors!(), about, long_about = None)]
#[command(help_template("\
{before-help}{name} {version}
Author: {author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
"))]
struct Args {
    /// The interval to run the command, in seconds (minimum 0.1)
    #[arg(name = "interval", short = 'n', long, value_name = "sec", default_value = "2.0")]
    interval: f64,

    /// Beep if command has a non-zero exit
    #[arg(short = 'b', long)]
    beep: bool,

    /// Interpret ANSI color and style sequences
    #[arg(short = 'c', long)]
    color: bool,

    /// Do not interpret ANSI color and style sequences
    #[arg(short = 'C', long = "no-color")]
    no_color: bool,

    /// Highlight differences between successive updates
    #[arg(short = 'd', long)]
    differences: bool,

    /// Highlight all changes since the first iteration (permanent diff)
    #[arg(long = "differences-permanent")]
    differences_permanent: bool,

    /// Freeze updates on command error, and exit after a key press
    #[arg(short = 'e', long)]
    errexit: bool,

    /// Exit when the output of command changes
    #[arg(short = 'g', long)]
    chgexit: bool,

    /// Make watch attempt to run command every interval seconds precisely
    #[arg(short = 'p', long)]
    precise: bool,

    /// Exit when output does not change for the given number of cycles
    #[arg(short = 'q', long, value_name = "cycles")]
    equexit: Option<u32>,

    /// Do not run the program on terminal resize
    #[arg(short = 'r', long = "no-rerun")]
    no_rerun: bool,

    /// Turn off the header showing interval, command, and current time
    #[arg(short = 't', long = "no-title")]
    no_title: bool,

    /// Turn off line wrapping (long lines will be truncated)
    #[arg(short = 'w', long = "no-wrap")]
    no_wrap: bool,

    /// Pass command to exec instead of shell
    #[arg(short = 'x', long)]
    exec: bool,

    /// Execute the command exactly N times, then exit
    #[arg(long = "count", value_name = "N", conflicts_with_all = ["duration", "until"])]
    count: Option<u64>,

    /// Execute for the specified duration (e.g., "1h30m", "45s", "2h")
    #[arg(long = "duration", value_name = "DURATION", conflicts_with_all = ["count", "until"])]
    duration: Option<String>,

    /// Execute until the specified date/time (ISO 8601 format: YYYY-MM-DDTHH:MM:SS)
    #[arg(long = "until", value_name = "DATETIME", conflicts_with_all = ["count", "duration"])]
    until: Option<String>,

    /// The command to run
    #[arg(name = "command", required = true)]
    command: String,

    /// Any number of arguments to pass to the `command`
    #[arg(name = "args", required = false)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Validate interval
    let interval = if args.interval < 0.1 { 0.1 } else { args.interval };

    // Determine execution limit
    let execution_limit = if let Some(count) = args.count {
        Some(ExecutionLimit::Count(count))
    } else if let Some(ref duration_str) = args.duration {
        match parse_duration(duration_str) {
            Ok(duration) => Some(ExecutionLimit::Duration(duration)),
            Err(e) => {
                eprintln!("Error parsing duration: {}", e);
                std::process::exit(1);
            }
        }
    } else if let Some(ref until_str) = args.until {
        match chrono::DateTime::parse_from_rfc3339(until_str)
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(until_str, "%Y-%m-%dT%H:%M:%S")
                    .map(|ndt| ndt.and_local_timezone(chrono::Local).unwrap().fixed_offset())
            })
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(until_str, "%Y-%m-%d %H:%M:%S")
                    .map(|ndt| ndt.and_local_timezone(chrono::Local).unwrap().fixed_offset())
            })
        {
            Ok(dt) => Some(ExecutionLimit::Until(dt.with_timezone(&chrono::Local))),
            Err(e) => {
                eprintln!("Error parsing datetime: {}. Use ISO 8601 format: YYYY-MM-DDTHH:MM:SS", e);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let options = WatchOptions {
        command: args.command,
        args: args.args,
        interval,
        beep: args.beep,
        color: args.color && !args.no_color,
        differences: args.differences || args.differences_permanent,
        differences_permanent: args.differences_permanent,
        errexit: args.errexit,
        chgexit: args.chgexit,
        precise: args.precise,
        equexit: args.equexit,
        no_rerun: args.no_rerun,
        no_title: args.no_title,
        no_wrap: args.no_wrap,
        exec: args.exec,
        execution_limit,
    };

    watch_rs::watch(options)
}

/// Parse a duration string like "1h30m", "45s", "2h", "30m5s"
fn parse_duration(s: &str) -> std::result::Result<std::time::Duration, String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return Err("Empty duration string".to_string());
    }

    let mut total_secs: u64 = 0;
    let mut current_num = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            current_num.push(c);
        } else {
            if current_num.is_empty() {
                return Err(format!("Invalid duration format: unexpected '{}'", c));
            }
            let num: u64 = current_num.parse().map_err(|_| "Invalid number")?;
            current_num.clear();

            match c {
                'h' => total_secs += num * 3600,
                'm' => total_secs += num * 60,
                's' => total_secs += num,
                'd' => total_secs += num * 86400,
                _ => return Err(format!("Unknown duration unit: '{}'", c)),
            }
        }
    }

    // Handle case where string ends with a number (assume seconds)
    if !current_num.is_empty() {
        let num: u64 = current_num.parse().map_err(|_| "Invalid number")?;
        total_secs += num;
    }

    if total_secs == 0 {
        return Err("Duration must be greater than 0".to_string());
    }

    Ok(std::time::Duration::from_secs(total_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), std::time::Duration::from_secs(30));
        assert_eq!(parse_duration("45").unwrap(), std::time::Duration::from_secs(45));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("2h").unwrap(), std::time::Duration::from_secs(7200));
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(parse_duration("1h30m").unwrap(), std::time::Duration::from_secs(5400));
        assert_eq!(parse_duration("1h30m45s").unwrap(), std::time::Duration::from_secs(5445));
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("1d").unwrap(), std::time::Duration::from_secs(86400));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("0").is_err());
    }
}
