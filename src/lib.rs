//! # watch-rs
//!
//! A Rust implementation of Linux's `watch` command with extended features.
//!
//! This library provides functionality to repeatedly execute a command and display its output,
//! similar to the Linux `watch` utility but with additional features like:
//!
//! - Diff highlighting with colors
//! - Execution limits (count, duration, or until a specific time)
//! - Progress bar display for limited executions
//! - Cross-platform support (Windows, Linux, macOS)
//!
//! ## Example
//!
//! ```no_run
//! use watch_rs::{watch, WatchOptions};
//!
//! let options = WatchOptions {
//!     command: "ls".to_string(),
//!     args: vec!["-la".to_string()],
//!     interval: 2.0,
//!     ..Default::default()
//! };
//! if let Err(err) = watch(options) {
//!     eprintln!("Error: {}", err);
//! }
//! ```

use std::{
    io::{stdout, Error, ErrorKind, Result, Write},
    process::Command,
    time::{Duration, Instant},
};

use chrono::{DateTime, Local};
use console::{style, Style};
use crossterm::{
    cursor::*,
    event::{poll, read, Event, KeyCode},
    execute, queue,
    style::*,
    terminal::*,
};
use indicatif::{ProgressBar, ProgressStyle};
use similar::{ChangeTag, TextDiff};

/// Execution limit modes for the watch command
#[derive(Debug, Clone)]
pub enum ExecutionLimit {
    /// Execute the command exactly N times
    Count(u64),
    /// Execute for the specified duration
    Duration(Duration),
    /// Execute until the specified datetime
    Until(DateTime<Local>),
}

/// Configuration options for the watch command
#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// The command to execute
    pub command: String,
    /// Arguments to pass to the command
    pub args: Vec<String>,
    /// Interval between executions in seconds
    pub interval: f64,
    /// Beep if command has a non-zero exit
    pub beep: bool,
    /// Interpret ANSI color and style sequences
    pub color: bool,
    /// Strip ANSI color and style sequences
    pub no_color: bool,
    /// Highlight differences between successive updates
    pub differences: bool,
    /// Show all changes since first iteration (permanent diff mode)
    pub differences_permanent: bool,
    /// Freeze updates on command error, and exit after a key press
    pub errexit: bool,
    /// Exit when the output of command changes
    pub chgexit: bool,
    /// Attempt to run command every interval seconds precisely
    pub precise: bool,
    /// Exit when output does not change for the given number of cycles
    pub equexit: Option<u32>,
    /// Do not run the program on terminal resize
    pub no_rerun: bool,
    /// Turn off the header showing interval, command, and current time
    pub no_title: bool,
    /// Turn off line wrapping (long lines will be truncated)
    pub no_wrap: bool,
    /// Pass command to exec instead of shell
    pub exec: bool,
    /// Limit the number of executions
    pub execution_limit: Option<ExecutionLimit>,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            interval: 2.0,
            beep: false,
            color: false,
            no_color: false,
            differences: false,
            differences_permanent: false,
            errexit: false,
            chgexit: false,
            precise: false,
            equexit: None,
            no_rerun: false,
            no_title: false,
            no_wrap: false,
            exec: false,
            execution_limit: None,
        }
    }
}

struct TerminalSession {
    no_wrap: bool,
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut out = stdout();
        if self.no_wrap {
            let _ = execute!(out, LeaveAlternateScreen, Show, EnableLineWrap);
        } else {
            let _ = execute!(out, LeaveAlternateScreen, Show);
        }
        let _ = disable_raw_mode();
    }
}

/// Formats the output with diff highlighting using console colors
fn format_diff(old: &str, new: &str, permanent: bool) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut result = String::new();

    let added_style = Style::new().green().bold();
    let removed_style = Style::new().red().bold();
    let unchanged_style = Style::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => {
                if permanent {
                    result.push_str(&format!(
                        "{}",
                        removed_style.apply_to(format!("-{}", change))
                    ));
                }
            }
            ChangeTag::Insert => {
                result.push_str(&format!("{}", added_style.apply_to(format!("+{}", change))));
            }
            ChangeTag::Equal => {
                result.push_str(&format!("{}", unchanged_style.apply_to(change.to_string())));
            }
        }
    }

    result
}

fn strip_ansi_codes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Highlights inline character differences for a cleaner view.
/// This function is provided for future use when inline character-level
/// diff highlighting is desired instead of line-level diffs.
#[allow(dead_code)]
fn format_diff_inline(old: &str, new: &str) -> String {
    let diff = TextDiff::from_chars(old, new);
    let mut result = String::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => {
                result.push_str(&format!(
                    "{}",
                    style(change.to_string()).red().on_white().bold()
                ));
            }
            ChangeTag::Insert => {
                result.push_str(&format!(
                    "{}",
                    style(change.to_string()).green().on_black().bold()
                ));
            }
            ChangeTag::Equal => {
                result.push_str(&change.to_string());
            }
        }
    }

    result
}

/// Beep the terminal
fn beep() {
    print!("\x07");
    let _ = stdout().flush();
}

/// Creates a progress bar for limited execution modes
fn create_progress_bar(total: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .expect("Invalid progress bar template")
        .progress_chars("#>-"),
    );
    pb.set_message(message.to_string());
    pb
}

/// Creates a progress bar for duration-based execution
fn create_duration_progress_bar(duration_secs: u64) -> ProgressBar {
    let pb = ProgressBar::new(duration_secs);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {elapsed}/{duration} {msg}",
        )
        .expect("Invalid progress bar template")
        .progress_chars("#>-"),
    );
    pb.set_message("Running...");
    pb
}

/// Uses `crossterm` to watch a command and print its output.
/// Allows the user to exit by pressing 'q' or 'Ctrl+C'.
///
/// # Arguments
///
/// * `options` - The watch configuration options
///
/// # Errors
///
/// Returns a `std::io::Error` if the command fails to execute.
pub fn watch(options: WatchOptions) -> Result<()> {
    let interval_duration = Duration::from_secs_f64(options.interval);

    let mut full_watch_command = options.command.clone();
    if !options.args.is_empty() {
        full_watch_command.push(' ');
        full_watch_command.push_str(&options.args.join(" "));
    }

    let (program, command_arg): (&str, &str) = if options.exec {
        // Direct exec mode
        ("", "")
    } else if cfg!(windows) {
        ("powershell", "-Command")
    } else {
        ("sh", "-c")
    };

    const QUIT_MSG: &str = "Press 'q' or 'Ctrl+C' to exit";

    // Track previous output for diff highlighting
    let mut previous_output: Option<String> = None;
    let mut first_output: Option<String> = None;
    let mut unchanged_count: u32 = 0;
    let mut execution_count: u64 = 0;
    let start_time = Instant::now();

    // Set up progress bar if we have an execution limit
    let progress_bar: Option<ProgressBar> = match &options.execution_limit {
        Some(ExecutionLimit::Count(count)) => Some(create_progress_bar(*count, "Executions")),
        Some(ExecutionLimit::Duration(duration)) => {
            Some(create_duration_progress_bar(duration.as_secs()))
        }
        Some(ExecutionLimit::Until(until)) => {
            let now = Local::now();
            if *until > now {
                let duration = (*until - now).num_seconds().max(1) as u64;
                Some(create_duration_progress_bar(duration))
            } else {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Target time is in the past",
                ));
            }
        }
        None => None,
    };

    enable_raw_mode()?;
    let enter_result = if options.no_wrap {
        execute!(stdout(), Hide, EnterAlternateScreen, DisableLineWrap)
    } else {
        execute!(stdout(), Hide, EnterAlternateScreen, EnableLineWrap)
    };
    if let Err(err) = enter_result {
        disable_raw_mode()?;
        return Err(err);
    }
    let terminal_session = TerminalSession {
        no_wrap: options.no_wrap,
    };

    let mut last_output = String::new();
    let mut last_error = String::new();
    let mut should_exit = false;

    'watch_loop: loop {
        let loop_start = Instant::now();

        // Check execution limits
        match &options.execution_limit {
            Some(ExecutionLimit::Count(max_count)) => {
                if execution_count >= *max_count {
                    should_exit = true;
                }
            }
            Some(ExecutionLimit::Duration(max_duration)) => {
                if start_time.elapsed() >= *max_duration {
                    should_exit = true;
                }
            }
            Some(ExecutionLimit::Until(until)) => {
                if Local::now() >= *until {
                    should_exit = true;
                }
            }
            None => {}
        }

        if should_exit {
            break 'watch_loop;
        }

        // Update progress bar
        if let Some(ref pb) = progress_bar {
            match &options.execution_limit {
                Some(ExecutionLimit::Count(_)) => {
                    pb.set_position(execution_count);
                }
                Some(ExecutionLimit::Duration(_)) | Some(ExecutionLimit::Until(_)) => {
                    pb.set_position(start_time.elapsed().as_secs());
                }
                None => {}
            }
        }

        // Clear screen and prepare header
        queue!(stdout(), Clear(ClearType::All), MoveTo(0, 0))?;

        // Display header if enabled
        if !options.no_title {
            let now = Local::now();
            let time_str = now.format("%Y-%m-%d %H:%M:%S").to_string();

            queue!(
                stdout(),
                Print("Every "),
                PrintStyledContent(format!("{:.1}s", options.interval).bold()),
                Print(": "),
                PrintStyledContent(full_watch_command.clone().cyan()),
            )?;

            // Position time on the right
            let term_width = size().map(|(w, _)| w).unwrap_or(80);
            let time_col = term_width.saturating_sub(time_str.len() as u16);
            queue!(
                stdout(),
                MoveToColumn(time_col),
                Print(&time_str),
                MoveToNextLine(2),
            )?;
        }

        // Execute command
        let output = if options.exec {
            // Direct exec mode - run command directly
            let mut cmd = Command::new(&options.command);
            cmd.args(&options.args);
            cmd.output()?
        } else {
            Command::new(program)
                .arg(command_arg)
                .arg(&full_watch_command)
                .output()?
        };

        execution_count += 1;

        // Handle command failure
        if !output.status.success() {
            if options.beep {
                beep();
            }
            if options.errexit {
                let error_msg = format!(
                    "Command exited with code: {}. Press any key to exit.",
                    output.status.code().unwrap_or(-1)
                );
                queue!(
                    stdout(),
                    PrintStyledContent(error_msg.red().bold()),
                    MoveToNextLine(1),
                )?;
                stdout().flush()?;

                // Wait for keypress
                loop {
                    if poll(Duration::from_millis(100))? {
                        let _ = read()?;
                        break 'watch_loop;
                    }
                }
            }
        }

        let stdout_content = String::from_utf8_lossy(&output.stdout);
        let stderr_content = String::from_utf8_lossy(&output.stderr);
        let current_output = stdout_content.trim().to_string();
        let current_error = stderr_content.trim().to_string();

        // Store first output for permanent diff mode
        if first_output.is_none() {
            first_output = Some(current_output.clone());
        }

        // Check for output change
        let output_changed = previous_output
            .as_ref()
            .is_some_and(|prev| prev != &current_output);

        if options.chgexit && output_changed {
            break 'watch_loop;
        }

        // Handle equexit
        if let Some(max_unchanged) = options.equexit {
            if output_changed {
                unchanged_count = 0;
            } else if previous_output.is_some() {
                unchanged_count += 1;
                if unchanged_count >= max_unchanged {
                    break 'watch_loop;
                }
            }
        }

        // Determine what to display
        let mut display_output = if options.differences {
            let compare_with = if options.differences_permanent {
                first_output.as_deref().unwrap_or("")
            } else {
                previous_output.as_deref().unwrap_or("")
            };
            if compare_with.is_empty() || compare_with == current_output {
                current_output.clone()
            } else {
                format_diff(compare_with, &current_output, options.differences_permanent)
            }
        } else {
            current_output.clone()
        };
        let display_error = if options.no_color {
            display_output = strip_ansi_codes(&display_output);
            strip_ansi_codes(&current_error)
        } else {
            current_error.clone()
        };

        // Print output
        queue!(
            stdout(),
            PrintStyledContent("Output:".bold().underlined()),
            MoveToNextLine(1),
        )?;

        if options.color {
            // Print with ANSI colors preserved
            queue!(stdout(), Print(&display_output))?;
        } else if options.differences && previous_output.is_some() {
            // Print diff-highlighted output (already formatted with console colors)
            queue!(stdout(), Print(&display_output))?;
        } else {
            queue!(stdout(), Print(&display_output))?;
        }

        queue!(stdout(), MoveToNextLine(1))?;

        // Print stderr if any
        if !display_error.is_empty() {
            queue!(
                stdout(),
                MoveToNextLine(1),
                PrintStyledContent("StdErr:".bold().underlined().red()),
                MoveToNextLine(1),
                PrintStyledContent(display_error.red()),
                MoveToNextLine(1),
            )?;
        }

        // Display progress bar info if in limited mode
        if progress_bar.is_some() {
            let (_, term_height) = size().unwrap_or((80, 24));
            queue!(stdout(), MoveTo(0, term_height - 3))?;

            match &options.execution_limit {
                Some(ExecutionLimit::Count(max_count)) => {
                    queue!(
                        stdout(),
                        PrintStyledContent(
                            format!("Progress: {}/{} executions", execution_count, max_count)
                                .cyan()
                                .bold()
                        ),
                    )?;
                }
                Some(ExecutionLimit::Duration(duration)) => {
                    let elapsed = start_time.elapsed();
                    let remaining = duration.saturating_sub(elapsed);
                    queue!(
                        stdout(),
                        PrintStyledContent(
                            format!(
                                "Elapsed: {:.1}s / {:.1}s (Remaining: {:.1}s)",
                                elapsed.as_secs_f64(),
                                duration.as_secs_f64(),
                                remaining.as_secs_f64()
                            )
                            .cyan()
                            .bold()
                        ),
                    )?;
                }
                Some(ExecutionLimit::Until(until)) => {
                    let now = Local::now();
                    let remaining = (*until - now).num_seconds().max(0);
                    queue!(
                        stdout(),
                        PrintStyledContent(
                            format!(
                                "Running until: {} (Remaining: {}s)",
                                until.format("%Y-%m-%d %H:%M:%S"),
                                remaining
                            )
                            .cyan()
                            .bold()
                        ),
                    )?;
                }
                None => {}
            }
        }

        // Display quit message
        let (term_width, term_height) = size().unwrap_or((80, 24));
        queue!(
            stdout(),
            MoveTo(
                term_width.saturating_sub(QUIT_MSG.len() as u16),
                term_height - 1
            ),
            PrintStyledContent(QUIT_MSG.italic()),
        )?;

        // Store current output for next iteration
        previous_output = Some(current_output.clone());
        last_output = current_output;
        last_error = current_error;

        stdout().flush()?;

        // Calculate sleep time for precise mode
        let target_wait = if options.precise {
            interval_duration.saturating_sub(loop_start.elapsed())
        } else {
            interval_duration
        };

        // Poll for keys/sleep
        let wait_start = Instant::now();
        while wait_start.elapsed() < target_wait {
            let remaining = target_wait.saturating_sub(wait_start.elapsed());
            if remaining.is_zero() {
                break;
            }

            if poll(remaining.min(Duration::from_millis(100)))? {
                match read()? {
                    Event::Key(event)
                        if event.code == KeyCode::Char('q')
                            || (event.code == KeyCode::Char('c')
                                && event.modifiers == crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        should_exit = true;
                        break;
                    }
                    Event::Resize(_, _) if !options.no_rerun => {
                        // Trigger immediate re-run on resize
                        break;
                    }
                    _ => {}
                }
            }

            // Update progress bar during wait
            if let Some(ref pb) = progress_bar {
                match &options.execution_limit {
                    Some(ExecutionLimit::Duration(_)) | Some(ExecutionLimit::Until(_)) => {
                        pb.set_position(start_time.elapsed().as_secs());
                    }
                    _ => {}
                }
            }
        }

        if should_exit {
            break 'watch_loop;
        }
    }

    // Finish progress bar
    if let Some(pb) = progress_bar {
        pb.finish_with_message("Done!");
    }

    // Leave alternate screen and print final output
    drop(terminal_session);

    queue!(
        stdout(),
        Print("> "),
        PrintStyledContent(full_watch_command.cyan()),
        MoveToNextLine(2),
        PrintStyledContent("Final Output:".bold().underlined()),
        MoveToNextLine(1),
        Print(&last_output),
        MoveToNextLine(1),
    )?;

    if !last_error.is_empty() {
        queue!(
            stdout(),
            PrintStyledContent("StdErr:".bold().underlined().red()),
            MoveToNextLine(1),
            PrintStyledContent(last_error.red()),
            MoveToNextLine(1),
        )?;
    }

    // Display execution summary for limited modes
    if options.execution_limit.is_some() {
        queue!(
            stdout(),
            MoveToNextLine(1),
            PrintStyledContent("─".repeat(40).dim()),
            MoveToNextLine(1),
            PrintStyledContent(
                format!("Total executions: {}", execution_count)
                    .green()
                    .bold()
            ),
            MoveToNextLine(1),
            PrintStyledContent(
                format!("Total time: {:.2}s", start_time.elapsed().as_secs_f64())
                    .green()
                    .bold()
            ),
            MoveToNextLine(1),
        )?;
    }

    stdout().flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let options = WatchOptions::default();
        assert_eq!(options.interval, 2.0);
        assert!(!options.beep);
        assert!(!options.color);
        assert!(!options.no_color);
        assert!(!options.differences);
    }

    #[test]
    fn test_format_diff_basic() {
        let old = "line1\nline2\n";
        let new = "line1\nline3\n";
        let result = format_diff(old, new, false);
        assert!(result.contains("line1"));
    }

    #[test]
    fn test_execution_limit_count() {
        let limit = ExecutionLimit::Count(5);
        match limit {
            ExecutionLimit::Count(n) => assert_eq!(n, 5),
            _ => panic!("Expected Count variant"),
        }
    }

    #[test]
    fn test_execution_limit_duration() {
        let limit = ExecutionLimit::Duration(Duration::from_secs(60));
        match limit {
            ExecutionLimit::Duration(d) => assert_eq!(d, Duration::from_secs(60)),
            _ => panic!("Expected Duration variant"),
        }
    }

    #[test]
    fn test_strip_ansi_codes() {
        assert_eq!(strip_ansi_codes("\x1b[31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi_codes("plain"), "plain");
    }
}
