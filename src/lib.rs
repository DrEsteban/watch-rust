//! Cross-platform implementation of the Linux `watch` command.
//!
//! The library repeatedly executes a command in a terminal and supports the
//! standard `watch` display, timing, comparison, and input modes. It also
//! supports execution limits by count, duration, or deadline.
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
//! watch(options)?;
//! # Ok::<(), std::io::Error>(())
//! ```

use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, stdout, IsTerminal, Read, Result, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anstyle_parse::{DefaultCharAccumulator, Params, Parser as AnsiParser, Perform};
use chrono::{DateTime, Local};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, ContentStyle, Print, PrintStyledContent, ResetColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, size, Clear, ClearType, DisableLineWrap, EnableLineWrap,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use unicode_width::UnicodeWidthChar;

const MIN_INTERVAL: f64 = 0.1;
const MAX_INTERVAL: f64 = 60.0 * 60.0 * 24.0 * 31.0;
const DEFAULT_WIDTH: u16 = 80;
const DEFAULT_HEIGHT: u16 = 24;
const TAB_WIDTH: usize = 8;
const MIN_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FOLLOW_ROWS: usize = 100_000;

/// A bound on how long the command should be watched.
#[derive(Debug, Clone)]
pub enum ExecutionLimit {
    /// Execute the command exactly this many times.
    Count(u64),
    /// Execute commands until this much monotonic time has elapsed.
    Duration(Duration),
    /// Execute commands until this local date and time.
    Until(DateTime<Local>),
}

/// Configuration for [`watch`] and [`watch_with_exit_code`].
#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Command to execute.
    pub command: String,
    /// Arguments passed to the command.
    pub args: Vec<String>,
    /// Interval between executions, in seconds.
    pub interval: f64,
    /// Beep when the command exits unsuccessfully.
    pub beep: bool,
    /// Interpret ANSI SGR color and style sequences.
    pub color: bool,
    /// Explicitly disable ANSI color and style interpretation.
    pub no_color: bool,
    /// Highlight visible changes between updates.
    pub differences: bool,
    /// Keep all visible change highlights since the first update.
    pub differences_permanent: bool,
    /// Freeze and return the command status when it exits unsuccessfully.
    pub errexit: bool,
    /// Append each update instead of clearing the screen.
    pub follow: bool,
    /// Exit when visible command output changes.
    pub chgexit: bool,
    /// Include command runtime in the interval.
    pub precise: bool,
    /// Exit after visible output is unchanged for this many cycles.
    pub equexit: Option<u32>,
    /// Wait for the next scheduled run after a terminal resize.
    pub no_rerun: bool,
    /// Directory used by the `s` screenshot key.
    pub shots_dir: Option<PathBuf>,
    /// Hide the two-line header.
    pub no_title: bool,
    /// Truncate long output lines instead of wrapping them.
    pub no_wrap: bool,
    /// Execute directly instead of through the platform shell.
    pub exec: bool,
    /// Optional extended execution limit.
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
            follow: false,
            chgexit: false,
            precise: false,
            equexit: None,
            no_rerun: false,
            shots_dir: None,
            no_title: false,
            no_wrap: false,
            exec: false,
            execution_limit: None,
        }
    }
}

struct TerminalSession {
    alternate_screen: bool,
}

impl TerminalSession {
    fn enter(output: &mut impl Write, follow: bool) -> Result<Self> {
        enable_raw_mode()?;
        let session = Self {
            alternate_screen: !follow,
        };

        let enter_result = if follow {
            execute!(output, Hide, DisableLineWrap)
        } else {
            execute!(
                output,
                Hide,
                EnterAlternateScreen,
                DisableLineWrap,
                Clear(ClearType::All)
            )
        };
        enter_result?;
        Ok(session)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut output = stdout();
        if self.alternate_screen {
            let _ = execute!(
                output,
                ResetColor,
                Show,
                EnableLineWrap,
                LeaveAlternateScreen
            );
        } else {
            let _ = execute!(output, ResetColor, Show, EnableLineWrap);
        }
        let _ = disable_raw_mode();
    }
}

#[derive(Clone, Debug)]
struct Cell {
    text: String,
    style: ContentStyle,
    width: u8,
    continuation: bool,
    occupied: bool,
}

impl Cell {
    fn blank() -> Self {
        Self {
            text: " ".to_string(),
            style: ContentStyle::default(),
            width: 1,
            continuation: false,
            occupied: false,
        }
    }

    fn glyph(text: String, style: ContentStyle, width: usize) -> Self {
        Self {
            text,
            style,
            width: width as u8,
            continuation: false,
            occupied: true,
        }
    }

    fn continuation(style: ContentStyle) -> Self {
        Self {
            text: String::new(),
            style,
            width: 0,
            continuation: true,
            occupied: true,
        }
    }
}

#[derive(Clone, Debug)]
struct Screen {
    lines: Vec<Vec<Cell>>,
    width: usize,
    height: usize,
}

impl Screen {
    fn parse(input: &[u8], width: usize, height: usize, no_wrap: bool, color: bool) -> Self {
        let mut parser = AnsiParser::<DefaultCharAccumulator>::new();
        let mut builder = ScreenBuilder::new(width, height, no_wrap, color);
        for &byte in input {
            parser.advance(&mut builder, byte);
        }
        builder.screen
    }

    fn cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.lines.get(row).and_then(|line| line.get(column))
    }

    fn plain_lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .take(self.height)
            .map(|line| {
                let end = line
                    .iter()
                    .rposition(|cell| cell.occupied)
                    .map_or(0, |index| index + 1);
                let mut rendered = String::new();
                for cell in &line[..end] {
                    if !cell.continuation {
                        rendered.push_str(&cell.text);
                    }
                }
                rendered
            })
            .collect()
    }
}

struct ScreenBuilder {
    screen: Screen,
    row: usize,
    column: usize,
    no_wrap: bool,
    color: bool,
    line_truncated: bool,
    style: ContentStyle,
}

impl ScreenBuilder {
    fn new(width: usize, height: usize, no_wrap: bool, color: bool) -> Self {
        Self {
            screen: Screen {
                lines: Vec::new(),
                width,
                height,
            },
            row: 0,
            column: 0,
            no_wrap,
            color,
            line_truncated: false,
            style: ContentStyle::default(),
        }
    }

    fn ensure_line(&mut self) {
        if self.row >= self.screen.height {
            return;
        }
        while self.screen.lines.len() <= self.row {
            self.screen.lines.push(Vec::new());
        }
    }

    fn next_line(&mut self) {
        self.ensure_line();
        self.row = self.row.saturating_add(1);
        self.column = 0;
        self.line_truncated = false;
        self.ensure_line();
    }

    fn put_space(&mut self) {
        self.put_character(' ');
    }

    fn put_character(&mut self, character: char) {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            self.append_combining(character);
            return;
        }
        if self.screen.width == 0 || self.row >= self.screen.height {
            return;
        }
        if self.line_truncated {
            return;
        }

        if self.column.saturating_add(width) > self.screen.width {
            if self.no_wrap {
                self.line_truncated = true;
                return;
            }
            self.next_line();
            if self.row >= self.screen.height {
                return;
            }
        }

        self.ensure_line();
        let line = &mut self.screen.lines[self.row];
        while line.len() < self.column.saturating_add(width) {
            line.push(Cell::blank());
        }
        clear_occupant(line, self.column);
        if width == 2 {
            clear_occupant(line, self.column + 1);
        }

        line[self.column] = Cell::glyph(character.to_string(), self.style, width);
        if width == 2 {
            line[self.column + 1] = Cell::continuation(self.style);
        }
        self.column += width;
    }

    fn append_combining(&mut self, character: char) {
        if self.row >= self.screen.lines.len() || self.column == 0 {
            return;
        }
        let line = &mut self.screen.lines[self.row];
        let mut index = self
            .column
            .saturating_sub(1)
            .min(line.len().saturating_sub(1));
        while index > 0 && line[index].continuation {
            index -= 1;
        }
        if let Some(cell) = line.get_mut(index).filter(|cell| cell.occupied) {
            cell.text.push(character);
        }
    }

    fn carriage_return(&mut self) {
        self.column = 0;
        self.line_truncated = false;
    }

    fn backspace(&mut self) {
        self.column = self.column.saturating_sub(1);
        if let Some(line) = self.screen.lines.get(self.row) {
            while self.column > 0 && line.get(self.column).is_some_and(|cell| cell.continuation) {
                self.column -= 1;
            }
        }
    }

    fn tab(&mut self) {
        let spaces = (TAB_WIDTH - (self.column % TAB_WIDTH))
            .min(self.screen.width.saturating_sub(self.column));
        for _ in 0..spaces {
            self.put_space();
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        if !self.color {
            return;
        }
        let mut values = params
            .iter()
            .flat_map(|parameter| parameter.iter().copied())
            .collect::<Vec<_>>();
        if values.is_empty() {
            values.push(0);
        }

        let mut index = 0;
        while index < values.len() {
            let value = values[index];
            match value {
                0 => self.style = ContentStyle::default(),
                1 => self.style.attributes.set(Attribute::Bold),
                2 => self.style.attributes.set(Attribute::Dim),
                3 => self.style.attributes.set(Attribute::Italic),
                4 => self.style.attributes.set(Attribute::Underlined),
                5 => self.style.attributes.set(Attribute::SlowBlink),
                6 => self.style.attributes.set(Attribute::RapidBlink),
                7 => self.style.attributes.set(Attribute::Reverse),
                8 => self.style.attributes.set(Attribute::Hidden),
                9 => self.style.attributes.set(Attribute::CrossedOut),
                21 => self.style.attributes.unset(Attribute::Bold),
                22 => {
                    self.style.attributes.unset(Attribute::Bold);
                    self.style.attributes.unset(Attribute::Dim);
                }
                23 => self.style.attributes.unset(Attribute::Italic),
                24 => self.style.attributes.unset(Attribute::Underlined),
                25 => {
                    self.style.attributes.unset(Attribute::SlowBlink);
                    self.style.attributes.unset(Attribute::RapidBlink);
                }
                27 => self.style.attributes.unset(Attribute::Reverse),
                28 => self.style.attributes.unset(Attribute::Hidden),
                29 => self.style.attributes.unset(Attribute::CrossedOut),
                30..=37 => self.style.foreground_color = Some(standard_color(value - 30, false)),
                38 => {
                    if let Some((color, consumed)) = extended_color(&values[index + 1..]) {
                        self.style.foreground_color = Some(color);
                        index += consumed;
                    }
                }
                39 => self.style.foreground_color = None,
                40..=47 => self.style.background_color = Some(standard_color(value - 40, false)),
                48 => {
                    if let Some((color, consumed)) = extended_color(&values[index + 1..]) {
                        self.style.background_color = Some(color);
                        index += consumed;
                    }
                }
                49 => self.style.background_color = None,
                90..=97 => self.style.foreground_color = Some(standard_color(value - 90, true)),
                100..=107 => {
                    self.style.background_color = Some(standard_color(value - 100, true));
                }
                _ => {}
            }
            index += 1;
        }
    }
}

impl Perform for ScreenBuilder {
    fn print(&mut self, character: char) {
        if !character.is_control() {
            self.put_character(character);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.next_line(),
            b'\r' => self.carriage_return(),
            b'\t' => self.tab(),
            0x08 => self.backspace(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
        if action == b'm' && intermediates.is_empty() && !ignore {
            self.apply_sgr(params);
        }
    }
}

fn clear_occupant(line: &mut [Cell], column: usize) {
    if column >= line.len() {
        return;
    }
    let start = if line[column].continuation {
        column.saturating_sub(1)
    } else {
        column
    };
    let width = line[start].width.max(1) as usize;
    for cell in line.iter_mut().skip(start).take(width) {
        *cell = Cell::blank();
    }
}

fn standard_color(index: u16, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::DarkRed,
        (2, false) => Color::DarkGreen,
        (3, false) => Color::DarkYellow,
        (4, false) => Color::DarkBlue,
        (5, false) => Color::DarkMagenta,
        (6, false) => Color::DarkCyan,
        (7, false) => Color::Grey,
        (0, true) => Color::DarkGrey,
        (1, true) => Color::Red,
        (2, true) => Color::Green,
        (3, true) => Color::Yellow,
        (4, true) => Color::Blue,
        (5, true) => Color::Magenta,
        (6, true) => Color::Cyan,
        _ => Color::White,
    }
}

fn extended_color(values: &[u16]) -> Option<(Color, usize)> {
    match values {
        [5, value, ..] if *value <= u8::MAX as u16 => Some((Color::AnsiValue(*value as u8), 2)),
        [2, red, green, blue, ..]
            if *red <= u8::MAX as u16 && *green <= u8::MAX as u16 && *blue <= u8::MAX as u16 =>
        {
            Some((
                Color::Rgb {
                    r: *red as u8,
                    g: *green as u8,
                    b: *blue as u8,
                },
                4,
            ))
        }
        _ => None,
    }
}

type DiffMask = Vec<Vec<bool>>;

fn screen_changes(current: &Screen, previous: Option<&Screen>) -> (bool, DiffMask) {
    let Some(previous) = previous else {
        return (false, Vec::new());
    };

    let rows = current.height.min(previous.height);
    let mut any_changed = false;
    let mut mask = Vec::with_capacity(rows);
    for row in 0..rows {
        let columns = current
            .lines
            .get(row)
            .map_or(0, Vec::len)
            .max(previous.lines.get(row).map_or(0, Vec::len))
            .min(current.width);
        let mut row_mask = Vec::with_capacity(columns);
        for column in 0..columns {
            let changed =
                !cells_visibly_equal(current.cell(row, column), previous.cell(row, column));
            any_changed |= changed;
            row_mask.push(changed);
        }
        mask.push(row_mask);
    }
    (any_changed, mask)
}

fn cells_visibly_equal(left: Option<&Cell>, right: Option<&Cell>) -> bool {
    visible_cell_signature(left) == visible_cell_signature(right)
}

fn visible_cell_signature(cell: Option<&Cell>) -> (&str, bool) {
    match cell {
        Some(cell) if cell.continuation => ("", true),
        Some(cell) if cell.occupied => (cell.text.as_str(), false),
        _ => (" ", false),
    }
}

fn merge_diff_masks(permanent: &mut DiffMask, current: &DiffMask) {
    if permanent.len() < current.len() {
        permanent.resize_with(current.len(), Vec::new);
    }
    for (target, source) in permanent.iter_mut().zip(current) {
        if target.len() < source.len() {
            target.resize(source.len(), false);
        }
        for (target, source) in target.iter_mut().zip(source) {
            *target |= *source;
        }
    }
}

#[derive(Clone, Copy)]
struct Layout {
    width: usize,
    height: usize,
    header_rows: usize,
    output_rows: usize,
    progress_rows: usize,
}

impl Layout {
    fn new(width: u16, height: u16, no_title: bool, has_progress: bool) -> Self {
        let width = width as usize;
        let height = height as usize;
        let header_rows = if no_title { 0 } else { 2.min(height) };
        let remaining = height.saturating_sub(header_rows);
        let progress_rows = usize::from(has_progress && remaining > 0);
        Self {
            width,
            height,
            header_rows,
            output_rows: remaining.saturating_sub(progress_rows),
            progress_rows,
        }
    }
}

fn render_fullscreen(
    output: &mut impl Write,
    screen: &Screen,
    highlights: Option<&DiffMask>,
    header: &[String],
    progress: Option<&str>,
    layout: Layout,
) -> Result<()> {
    queue!(output, Clear(ClearType::All), MoveTo(0, 0))?;

    for (row, line) in header.iter().take(layout.header_rows).enumerate() {
        queue!(
            output,
            MoveTo(0, row as u16),
            Print(truncate_to_width(line, layout.width))
        )?;
    }
    render_screen(
        output,
        screen,
        highlights,
        layout.header_rows,
        layout.output_rows,
    )?;

    if layout.progress_rows == 1 {
        if let Some(progress) = progress {
            queue!(
                output,
                MoveTo(0, layout.height.saturating_sub(1) as u16),
                Print(truncate_to_width(progress, layout.width))
            )?;
        }
    }
    Ok(())
}

fn render_follow(
    output: &mut impl Write,
    screen: &Screen,
    header: &[String],
    progress: Option<&str>,
) -> Result<()> {
    for line in header {
        queue!(output, Print(line), Print("\r\n"))?;
    }
    render_screen(output, screen, None, 0, screen.height)?;
    if screen.lines.is_empty() {
        queue!(output, Print("\r\n"))?;
    }
    if let Some(progress) = progress {
        queue!(output, Print(progress), Print("\r\n"))?;
    }
    Ok(())
}

fn render_screen(
    output: &mut impl Write,
    screen: &Screen,
    highlights: Option<&DiffMask>,
    row_offset: usize,
    max_rows: usize,
) -> Result<()> {
    for row in 0..screen.lines.len().min(max_rows) {
        if row_offset > 0 {
            queue!(output, MoveTo(0, (row_offset + row) as u16))?;
        }

        let highlight_columns = highlights
            .and_then(|mask| mask.get(row))
            .map_or(0, Vec::len);
        let columns = screen.lines[row]
            .len()
            .max(highlight_columns)
            .min(screen.width);
        let mut segment = String::new();
        let mut segment_style: Option<ContentStyle> = None;

        for column in 0..columns {
            let cell = screen.cell(row, column);
            if cell.is_some_and(|cell| cell.continuation) {
                continue;
            }

            let mut style = cell.map_or_else(ContentStyle::default, |cell| cell.style);
            let highlighted = highlights
                .and_then(|mask| mask.get(row))
                .and_then(|line| line.get(column))
                .copied()
                .unwrap_or(false)
                || cell.is_some_and(|cell| {
                    cell.width == 2
                        && highlights
                            .and_then(|mask| mask.get(row))
                            .and_then(|line| line.get(column + 1))
                            .copied()
                            .unwrap_or(false)
                });
            if highlighted {
                style.attributes.set(Attribute::Reverse);
            }

            let text = cell
                .filter(|cell| cell.occupied)
                .map_or(" ", |cell| cell.text.as_str());
            if segment_style.is_some_and(|current| current != style) {
                flush_segment(output, &mut segment, segment_style.take().unwrap())?;
            }
            segment_style.get_or_insert(style);
            segment.push_str(text);
        }

        if let Some(style) = segment_style {
            flush_segment(output, &mut segment, style)?;
        }
        if row_offset == 0 {
            queue!(output, Print("\r\n"))?;
        }
    }
    Ok(())
}

fn flush_segment(output: &mut impl Write, segment: &mut String, style: ContentStyle) -> Result<()> {
    if !segment.is_empty() {
        queue!(output, PrintStyledContent(style.apply(segment.as_str())))?;
        segment.clear();
    }
    Ok(())
}

fn build_header(
    options: &WatchOptions,
    command: &str,
    interval: f64,
    run: &CommandRun,
    width: usize,
) -> Vec<String> {
    if options.no_title {
        return Vec::new();
    }
    let left = format!("Every {interval:.1}s: {}", printable_text(command));
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let host = env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .unwrap_or_default();
    let right = if host.is_empty() {
        timestamp.to_string()
    } else {
        format!("{host}: {timestamp}")
    };
    let status = format!(
        "in {:.3}s (exit {})",
        run.elapsed.as_secs_f64(),
        status_code(&run.status)
    );
    vec![
        fit_sides(&left, &right, width),
        fit_sides("", &status, width),
    ]
}

fn printable_text(input: &str) -> String {
    input
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

fn fit_sides(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let right = truncate_to_width(right, width);
    let right_width = display_width(&right);
    if right_width >= width {
        return right;
    }
    let left_width_limit = width.saturating_sub(right_width + 1);
    let left = truncate_to_width(left, left_width_limit);
    let spacing = width.saturating_sub(display_width(&left) + right_width);
    format!("{left}{}{right}", " ".repeat(spacing))
}

fn truncate_to_width(input: &str, width: usize) -> String {
    if display_width(input) <= width {
        return input.to_string();
    }
    if width == 0 {
        return String::new();
    }

    let target = width.saturating_sub(1);
    let mut result = String::new();
    let mut used = 0;
    for character in input.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > target {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

fn display_width(input: &str) -> usize {
    input
        .chars()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}

struct CommandRun {
    output: Vec<u8>,
    status: ExitStatus,
    elapsed: Duration,
}

fn execute_command(
    options: &WatchOptions,
    shell_command: &str,
    width: u16,
    height: u16,
    capture_limit: usize,
) -> Result<CommandRun> {
    let mut command = if options.exec {
        let mut command = Command::new(&options.command);
        command.args(&options.args);
        command
    } else if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            shell_command,
        ]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", shell_command]);
        command
    };

    command
        .env("COLUMNS", width.to_string())
        .env("LINES", height.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = command.spawn()?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stdout"))?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture command stderr"))?;

    let stdout_reader = thread::spawn(move || read_limited(child_stdout, capture_limit));
    let stderr_reader = thread::spawn(move || read_limited(child_stderr, capture_limit));
    let status = child.wait()?;
    let (mut command_output, stdout_truncated) = join_reader(stdout_reader)?;
    let (error_output, stderr_truncated) = join_reader(stderr_reader)?;

    if !command_output.is_empty() && !error_output.is_empty() && !command_output.ends_with(b"\n") {
        command_output.push(b'\n');
    }
    command_output.extend(error_output);
    if stdout_truncated || stderr_truncated {
        if !command_output.ends_with(b"\n") {
            command_output.push(b'\n');
        }
        command_output.extend_from_slice(b"[output truncated]\n");
    }

    Ok(CommandRun {
        output: command_output,
        status,
        elapsed: started.elapsed(),
    })
}

fn read_limited(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(MIN_CAPTURE_BYTES));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let kept = read.min(remaining);
        output.extend_from_slice(&buffer[..kept]);
        truncated |= kept < read;
    }
    Ok((output, truncated))
}

fn join_reader(reader: thread::JoinHandle<Result<(Vec<u8>, bool)>>) -> Result<(Vec<u8>, bool)> {
    reader
        .join()
        .map_err(|_| io::Error::other("command output reader panicked"))?
}

fn capture_limit(width: u16, height: u16, follow: bool) -> usize {
    if follow {
        return MAX_CAPTURE_BYTES;
    }
    usize::from(width)
        .saturating_mul(usize::from(height))
        .saturating_mul(16)
        .clamp(MIN_CAPTURE_BYTES, MAX_CAPTURE_BYTES)
}

fn status_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

fn beep(output: &mut impl Write) -> Result<()> {
    output.write_all(b"\x07")?;
    output.flush()
}

fn terminal_dimensions() -> (u16, u16) {
    let detected = size().unwrap_or((0, 0));
    normalize_dimensions(
        environment_dimension("COLUMNS").unwrap_or(detected.0),
        environment_dimension("LINES").unwrap_or(detected.1),
    )
}

fn environment_dimension(name: &str) -> Option<u16> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
}

fn normalize_dimensions(width: u16, height: u16) -> (u16, u16) {
    (
        if width == 0 { DEFAULT_WIDTH } else { width },
        if height == 0 { DEFAULT_HEIGHT } else { height },
    )
}

fn validate_options(options: &WatchOptions) -> Result<f64> {
    if options.command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command cannot be empty",
        ));
    }
    if !options.interval.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interval must be finite",
        ));
    }
    if options.follow && (options.differences || options.chgexit || options.equexit.is_some()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "follow mode cannot track screen changes",
        ));
    }
    if options.equexit == Some(0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "equexit cycles must be greater than zero",
        ));
    }
    match &options.execution_limit {
        Some(ExecutionLimit::Count(0)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "execution count must be greater than zero",
            ));
        }
        Some(ExecutionLimit::Duration(duration)) if duration.is_zero() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "duration must be greater than zero",
            ));
        }
        Some(ExecutionLimit::Until(until)) if *until <= Local::now() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target time must be in the future",
            ));
        }
        _ => {}
    }
    if let Some(directory) = &options.shots_dir {
        if !fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "screenshot directory does not exist or is not a directory: {}",
                    directory.display()
                ),
            ));
        }
    }
    Ok(options.interval.clamp(MIN_INTERVAL, MAX_INTERVAL))
}

fn full_command(options: &WatchOptions) -> String {
    std::iter::once(options.command.as_str())
        .chain(options.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn limit_reached(limit: Option<&ExecutionLimit>, execution_count: u64, started: Instant) -> bool {
    match limit {
        Some(ExecutionLimit::Count(maximum)) => execution_count >= *maximum,
        Some(ExecutionLimit::Duration(duration)) => started.elapsed() >= *duration,
        Some(ExecutionLimit::Until(until)) => Local::now() >= *until,
        None => false,
    }
}

fn remaining_limit(limit: Option<&ExecutionLimit>, started: Instant) -> Option<Duration> {
    match limit {
        Some(ExecutionLimit::Count(_)) | None => None,
        Some(ExecutionLimit::Duration(duration)) => {
            Some(duration.saturating_sub(started.elapsed()))
        }
        Some(ExecutionLimit::Until(until)) => (*until - Local::now()).to_std().ok(),
    }
}

fn progress_line(
    limit: Option<&ExecutionLimit>,
    execution_count: u64,
    started: Instant,
    initial_deadline_duration: Option<Duration>,
    width: usize,
) -> Option<String> {
    let (ratio, label) = match limit? {
        ExecutionLimit::Count(total) => (
            execution_count as f64 / *total as f64,
            format!("{execution_count}/{total} runs"),
        ),
        ExecutionLimit::Duration(total) => (
            started.elapsed().as_secs_f64() / total.as_secs_f64(),
            format!(
                "{:.1}/{:.1}s",
                started.elapsed().as_secs_f64().min(total.as_secs_f64()),
                total.as_secs_f64()
            ),
        ),
        ExecutionLimit::Until(until) => {
            let total = initial_deadline_duration.unwrap_or_default();
            let remaining = (*until - Local::now()).to_std().unwrap_or_default();
            let elapsed = total.saturating_sub(remaining);
            (
                elapsed.as_secs_f64() / total.as_secs_f64().max(f64::EPSILON),
                format!("until {}", until.format("%Y-%m-%d %H:%M:%S")),
            )
        }
    };
    Some(format_progress(ratio.clamp(0.0, 1.0), &label, width))
}

fn format_progress(ratio: f64, label: &str, width: usize) -> String {
    if width < 8 {
        return truncate_to_width(label, width);
    }
    let label = truncate_to_width(label, width.saturating_sub(5));
    let bar_width = width.saturating_sub(display_width(&label) + 3);
    let completed = ((bar_width as f64) * ratio).round() as usize;
    format!(
        "[{}{}] {label}",
        "#".repeat(completed.min(bar_width)),
        "-".repeat(bar_width.saturating_sub(completed))
    )
}

fn frame_lines(
    header: &[String],
    screen: &Screen,
    progress: Option<&str>,
    layout: Layout,
) -> Vec<String> {
    let mut lines = header
        .iter()
        .take(layout.header_rows)
        .map(|line| truncate_to_width(line, layout.width))
        .collect::<Vec<_>>();
    lines.extend(screen.plain_lines().into_iter().take(layout.output_rows));
    if layout.progress_rows == 1 {
        lines.push(truncate_to_width(
            progress.unwrap_or_default(),
            layout.width,
        ));
    }
    lines
}

fn save_screenshot(directory: Option<&Path>, lines: &[String]) -> Result<PathBuf> {
    let directory = directory.unwrap_or_else(|| Path::new("."));
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    for sequence in 0..=999 {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!("-{sequence:03}")
        };
        let path = directory.join(format!("watch_{timestamp}{suffix}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                for line in lines {
                    writeln!(file, "{line}")?;
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many screenshots were created in one second",
    ))
}

enum WaitOutcome {
    Elapsed,
    RunNow,
    Quit,
    Resize,
}

fn wait_for_next(
    duration: Duration,
    no_rerun: bool,
    shots_dir: Option<&Path>,
    frame: &[String],
) -> Result<WaitOutcome> {
    let started = Instant::now();
    loop {
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(WaitOutcome::Elapsed);
        }
        if !poll(remaining.min(Duration::from_millis(100)))? {
            continue;
        }
        match read()? {
            Event::Key(event) if key_is_active(event) => {
                if is_quit_key(event) {
                    return Ok(WaitOutcome::Quit);
                }
                match event.code {
                    KeyCode::Char(' ') => return Ok(WaitOutcome::RunNow),
                    KeyCode::Char('s') => {
                        save_screenshot(shots_dir, frame)?;
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) if !no_rerun => return Ok(WaitOutcome::Resize),
            _ => {}
        }
    }
}

fn wait_for_error_key(shots_dir: Option<&Path>, frame: &[String]) -> Result<()> {
    loop {
        if !poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(event) = read()? {
            if key_is_active(event) {
                if event.code == KeyCode::Char('s') {
                    save_screenshot(shots_dir, frame)?;
                }
                return Ok(());
            }
        }
    }
}

fn key_is_active(event: KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn is_quit_key(event: KeyEvent) -> bool {
    event.code == KeyCode::Char('q')
        || (event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL))
}

fn print_final_summary(
    output: &mut impl Write,
    command: &str,
    screen: Option<&Screen>,
    execution_count: u64,
    elapsed: Duration,
    include_output: bool,
) -> Result<()> {
    writeln!(output, "> {command}")?;
    if include_output {
        writeln!(output)?;
        if let Some(screen) = screen {
            for line in screen.plain_lines() {
                writeln!(output, "{line}")?;
            }
        }
    }
    writeln!(output)?;
    writeln!(output, "Total executions: {execution_count}")?;
    writeln!(output, "Total time: {:.2}s", elapsed.as_secs_f64())?;
    output.flush()
}

/// Runs `watch` and discards the status returned by `--errexit`.
///
/// Use [`watch_with_exit_code`] when the caller needs to propagate a watched
/// command's non-zero status.
pub fn watch(options: WatchOptions) -> Result<()> {
    watch_with_exit_code(options).map(|_| ())
}

/// Runs `watch` and returns the process status to use for the caller.
///
/// Normal termination returns zero. With [`WatchOptions::errexit`], a failed
/// command returns its exit code, or `128 + signal` on Unix.
pub fn watch_with_exit_code(options: WatchOptions) -> Result<i32> {
    let interval = validate_options(&options)?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "an interactive terminal is required",
        ));
    }

    let interval_duration = Duration::from_secs_f64(interval);
    let command = full_command(&options);
    let started = Instant::now();
    let initial_deadline_duration = match &options.execution_limit {
        Some(ExecutionLimit::Until(until)) => (*until - Local::now()).to_std().ok(),
        _ => None,
    };

    let mut output = stdout();
    let terminal_session = TerminalSession::enter(&mut output, options.follow)?;
    let mut previous_screen: Option<Screen> = None;
    let mut permanent_highlights = DiffMask::new();
    let mut unchanged_count = 0_u32;
    let mut execution_count = 0_u64;
    let mut last_screen: Option<Screen> = None;
    let mut return_code = 0;

    loop {
        if limit_reached(options.execution_limit.as_ref(), execution_count, started) {
            break;
        }

        let (width, height) = terminal_dimensions();
        let layout = Layout::new(
            width,
            height,
            options.no_title,
            options.execution_limit.is_some(),
        );
        let run_started = Instant::now();
        let run = execute_command(
            &options,
            &command,
            width,
            height,
            capture_limit(width, height, options.follow),
        )?;
        execution_count += 1;

        if !run.status.success() && options.beep {
            beep(&mut output)?;
        }

        let screen = Screen::parse(
            &run.output,
            layout.width,
            if options.follow {
                MAX_FOLLOW_ROWS
            } else {
                layout.output_rows
            },
            options.no_wrap,
            options.color && !options.no_color,
        );
        let (changed, changes) = screen_changes(&screen, previous_screen.as_ref());
        if changed {
            unchanged_count = 0;
        } else if previous_screen.is_some() {
            unchanged_count = unchanged_count.saturating_add(1);
        }

        if options.differences_permanent {
            merge_diff_masks(&mut permanent_highlights, &changes);
        }
        let highlights = if options.differences_permanent {
            Some(&permanent_highlights)
        } else if options.differences {
            Some(&changes)
        } else {
            None
        };

        let header = build_header(&options, &command, interval, &run, layout.width);
        let progress = progress_line(
            options.execution_limit.as_ref(),
            execution_count,
            started,
            initial_deadline_duration,
            layout.width,
        );
        if options.follow {
            render_follow(&mut output, &screen, &header, progress.as_deref())?;
        } else {
            render_fullscreen(
                &mut output,
                &screen,
                highlights,
                &header,
                progress.as_deref(),
                layout,
            )?;
        }
        let current_frame = frame_lines(&header, &screen, progress.as_deref(), layout);
        output.flush()?;
        last_screen = Some(screen.clone());

        if options.errexit && !run.status.success() {
            return_code = status_code(&run.status);
            wait_for_error_key(options.shots_dir.as_deref(), &current_frame)?;
            break;
        }
        if options.chgexit && changed {
            break;
        }
        if options
            .equexit
            .is_some_and(|maximum| unchanged_count >= maximum)
        {
            break;
        }
        if limit_reached(options.execution_limit.as_ref(), execution_count, started) {
            break;
        }

        previous_screen = Some(screen);
        let mut wait_duration = if options.precise {
            interval_duration.saturating_sub(run_started.elapsed())
        } else {
            interval_duration
        };
        if let Some(remaining) = remaining_limit(options.execution_limit.as_ref(), started) {
            wait_duration = wait_duration.min(remaining);
        }

        match wait_for_next(
            wait_duration,
            options.no_rerun,
            options.shots_dir.as_deref(),
            &current_frame,
        )? {
            WaitOutcome::Quit => break,
            WaitOutcome::Resize => {
                previous_screen = None;
                permanent_highlights.clear();
                unchanged_count = 0;
            }
            WaitOutcome::Elapsed | WaitOutcome::RunNow => {}
        }
    }

    drop(terminal_session);
    if options.execution_limit.is_some() {
        print_final_summary(
            &mut output,
            &printable_text(&command),
            last_screen.as_ref(),
            execution_count,
            started.elapsed(),
            !options.follow,
        )?;
    }
    Ok(return_code)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn plain(screen: &Screen) -> Vec<String> {
        screen.plain_lines()
    }

    #[test]
    fn default_options_match_watch_defaults() {
        let options = WatchOptions::default();
        assert_eq!(options.interval, 2.0);
        assert!(!options.color);
        assert!(!options.follow);
        assert!(!options.no_wrap);
        assert!(options.execution_limit.is_none());
    }

    #[test]
    fn options_validate_and_clamp_interval() {
        let mut options = WatchOptions {
            command: "echo".to_string(),
            interval: 0.0,
            ..WatchOptions::default()
        };
        assert_eq!(validate_options(&options).unwrap(), MIN_INTERVAL);
        options.interval = f64::NAN;
        assert_eq!(
            validate_options(&options).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn follow_rejects_change_tracking() {
        let options = WatchOptions {
            command: "echo".to_string(),
            follow: true,
            differences: true,
            ..WatchOptions::default()
        };
        assert!(validate_options(&options).is_err());
    }

    #[test]
    fn parser_strips_unsafe_sequences_and_controls() {
        let screen = Screen::parse(b"\x1b[31mred\x1b[2J!\x07", 20, 2, false, false);
        assert_eq!(plain(&screen), ["red!"]);
        assert_eq!(screen.cell(0, 0).unwrap().style, ContentStyle::default());
    }

    #[test]
    fn parser_interprets_only_sgr_when_color_is_enabled() {
        let screen = Screen::parse(b"\x1b[31mred\x1b[2J!", 20, 2, false, true);
        assert_eq!(plain(&screen), ["red!"]);
        assert_eq!(
            screen.cell(0, 0).unwrap().style.foreground_color,
            Some(Color::DarkRed)
        );
    }

    #[test]
    fn parser_expands_tabs_and_wraps_at_display_width() {
        let wrapped = Screen::parse(b"a\tb", 4, 3, false, false);
        assert_eq!(plain(&wrapped), ["a   ", "b"]);

        let truncated = Screen::parse(b"a\tb", 4, 3, true, false);
        assert_eq!(plain(&truncated), ["a   "]);
    }

    #[test]
    fn parser_accounts_for_wide_and_combining_characters() {
        let screen = Screen::parse("界e\u{301}".as_bytes(), 4, 2, false, false);
        assert_eq!(plain(&screen), ["界e\u{301}"]);
        assert!(screen.cell(0, 1).unwrap().continuation);
    }

    #[test]
    fn comparisons_are_limited_to_the_visible_screen() {
        let first = Screen::parse(b"same\nhidden-a", 10, 1, false, false);
        let second = Screen::parse(b"same\nhidden-b", 10, 1, false, false);
        assert!(!screen_changes(&second, Some(&first)).0);

        let changed = Screen::parse(b"different", 10, 1, false, false);
        assert!(screen_changes(&changed, Some(&first)).0);
    }

    #[test]
    fn comparisons_preserve_significant_whitespace() {
        let first = Screen::parse(b" value ", 10, 1, false, false);
        let second = Screen::parse(b"value", 10, 1, false, false);
        assert!(screen_changes(&second, Some(&first)).0);
    }

    #[test]
    fn permanent_differences_accumulate() {
        let first = Screen::parse(b"abc", 10, 1, false, false);
        let second = Screen::parse(b"axc", 10, 1, false, false);
        let third = Screen::parse(b"ayz", 10, 1, false, false);
        let (_, first_change) = screen_changes(&second, Some(&first));
        let (_, second_change) = screen_changes(&third, Some(&second));
        let mut permanent = DiffMask::new();
        merge_diff_masks(&mut permanent, &first_change);
        merge_diff_masks(&mut permanent, &second_change);
        assert_eq!(permanent, [vec![false, true, true]]);
    }

    #[test]
    fn limited_reader_drains_but_bounds_memory() {
        let (output, truncated) = read_limited(Cursor::new(vec![b'x'; 1024]), 100).unwrap();
        assert_eq!(output.len(), 100);
        assert!(truncated);
    }

    #[test]
    fn progress_fits_the_available_width() {
        let progress = format_progress(0.5, "5/10 runs", 20);
        assert!(display_width(&progress) <= 20);
        assert!(progress.contains('#'));
        assert!(display_width(&format_progress(0.5, "long", 4)) <= 4);
    }

    #[test]
    fn zero_terminal_dimensions_use_safe_defaults() {
        assert_eq!(normalize_dimensions(0, 0), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        let layout = Layout::new(1, 1, false, true);
        assert_eq!(layout.output_rows, 0);
    }

    #[test]
    fn count_limit_is_reached_without_an_extra_wait() {
        let started = Instant::now();
        let limit = ExecutionLimit::Count(1);
        assert!(!limit_reached(Some(&limit), 0, started));
        assert!(limit_reached(Some(&limit), 1, started));
    }

    #[test]
    fn screenshot_names_do_not_overwrite_existing_files() {
        let directory = env::temp_dir().join(format!(
            "watch-rs-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let first = save_screenshot(Some(&directory), &["first".to_string()]).unwrap();
        let second = save_screenshot(Some(&directory), &["second".to_string()]).unwrap();
        assert_ne!(first, second);
        fs::remove_dir_all(directory).unwrap();
    }
}
