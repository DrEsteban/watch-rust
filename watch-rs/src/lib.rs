use std::{
    io::{stdout, Error, Result, Write},
    process::Command,
    time::{Duration, Instant},
};

use crossterm::{
    cursor::*,
    event::{poll, read, Event, KeyCode},
    execute, queue,
    style::*,
    terminal::*,
};

/// Uses `crossterm` to watch a command and print its output.
/// Allows the user to exit by pressing 'q' or 'Ctrl+C'.
///
/// # Arguments
///
/// * `command` - The command to watch.
/// * `args` - The arguments to pass to the command.
/// * `interval` - The interval in seconds between command executions.
///
/// # Errors
///
/// Returns a `std::io::Error` if the command fails to execute.
///
/// # Examples
///
/// ```
/// use watch_rs::watch;
///
/// fn main() {
///     if let Err(err) = watch("ls".to_string(), vec!["-l".to_string()], 1) {
///         eprintln!("Error: {}", err);
///     }
/// }
/// ```
pub fn watch(command: String, args: Vec<String>, interval: u64) -> Result<()> {
    let interval_duration: Duration = Duration::from_secs(interval);

    let mut full_watch_command: String = command.to_owned();
    full_watch_command.push_str(" ");
    full_watch_command.push_str(args.join(" ").as_str());

    let (program, command_arg): (&str, &str);
    if cfg!(windows) {
        program = "powershell";
        command_arg = "-Command";
    } else {
        program = "sh";
        command_arg = "-c";
    }

    const QUIT_MSG: &str = "Press 'q' or 'Ctrl+C' to exit";
    let interval_msg = format!("Interval: {}s", interval);

    enable_raw_mode()?;
    execute!(stdout(), Hide, EnterAlternateScreen, EnableLineWrap)?;
    'watchLoop: loop {
        // Begin queueing updates
        queue!(
            stdout(),
            Clear(ClearType::All),
            MoveTo(0, 0),
            Print("> "),
            PrintStyledContent(full_watch_command.to_owned().rapid_blink()),
            MoveToColumn(size().unwrap().0 - interval_msg.len() as u16),
            PrintStyledContent(interval_msg.to_owned().bold()),
            MoveToNextLine(2),
        )?;
        let output = Command::new(program)
            .arg(command_arg)
            .arg(&full_watch_command)
            .output()?;

        if !output.status.success() {
            return Err(Error::other(format!(
                "Command failed with exitCode: {}",
                output.status.code().unwrap()
            )));
        }

        let to_trim = String::from_utf8(output.stdout).expect("Get stdout");
        let std_output = to_trim.trim();
        let to_trim = String::from_utf8(output.stderr).expect("Get stderr");
        let std_error = to_trim.trim();

        // Print the output
        queue!(
            stdout(),
            PrintStyledContent("Output:".bold().underlined()),
            MoveToNextLine(1),
            Print(std_output),
            MoveToNextLine(1),
        )?;
        if !std_error.is_empty() {
            queue!(
                stdout(),
                PrintStyledContent("StdErr:".bold().underlined()),
                MoveToNextLine(1),
                Print(std_error),
                MoveToNextLine(1),
            )?;
        }
        queue!(
            stdout(),
            MoveTo(size().unwrap().0 - QUIT_MSG.len() as u16, size().unwrap().1 - 1),
            PrintStyledContent(QUIT_MSG.italic()),
        )?;

        // Flush updates
        stdout().flush()?;

        // Poll for keys/sleep
        let start_time = Instant::now();
        while start_time.elapsed() < interval_duration {
            if poll(interval_duration - start_time.elapsed())? {
                match read()? {
                    Event::Key(event)
                        if event.code == KeyCode::Char('q')
                            || (event.code == KeyCode::Char('c')
                                && event.modifiers == crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        // Leave alternate screen and print output one more time before exit
                        queue!(
                            stdout(),
                            LeaveAlternateScreen,
                            Print("> "),
                            Print(full_watch_command),
                            MoveToNextLine(2),
                            PrintStyledContent("Output:".bold().underlined()),
                            MoveToNextLine(1),
                            Print(std_output),
                            MoveToNextLine(1),
                        )?;
                        if !std_error.is_empty() {
                            queue!(
                                stdout(),
                                PrintStyledContent("StdErr:".bold().underlined()),
                                MoveToNextLine(1),
                                Print(std_error),
                                MoveToNextLine(1),
                            )?;
                        }
                        stdout().flush()?;
                        break 'watchLoop;
                    }
                    _ => {}
                }
            }
        }
    }
    execute!(stdout(), Show, DisableLineWrap)?;
    disable_raw_mode()
}
