# watch-rust (watch-rs)

A powerful cross-platform command-line tool similar to Linux's `watch`, written in Rust!

This tool repeatedly executes a command and displays its output, allowing you to watch the program output change over time. It includes all standard features from the Linux `watch` command plus extended functionality.

## Features

### Standard Linux `watch` Features
- **Interval control** (`-n, --interval`): Specify update interval in seconds (minimum 0.1s)
- **Beep on error** (`-b, --beep`): Audible alert if command returns non-zero exit code
- **Color support** (`-c, --color`): Interpret ANSI color and style sequences
- **No color** (`-C, --no-color`): Strip ANSI color sequences from output
- **Diff highlighting** (`-d, --differences`): Highlight changes between successive updates with colors
- **Permanent diff** (`--differences-permanent`): Show all changes since the first iteration
- **Exit on error** (`-e, --errexit`): Freeze on command error and exit after keypress
- **Exit on change** (`-g, --chgexit`): Exit when the output changes
- **Precise timing** (`-p, --precise`): Attempt to run command every interval seconds precisely
- **Exit on unchanged** (`-q, --equexit`): Exit when output doesn't change for N cycles
- **No rerun on resize** (`-r, --no-rerun`): Don't rerun command when terminal is resized
- **No title** (`-t, --no-title`): Hide the header showing interval, command, and time
- **No line wrap** (`-w, --no-wrap`): Truncate long lines instead of wrapping
- **Exec mode** (`-x, --exec`): Pass command directly to exec instead of shell

### Extended Features
- **Count mode** (`--count N`): Execute the command exactly N times, then exit
- **Duration mode** (`--duration DURATION`): Execute for a specified time span (e.g., "1h30m", "45s", "2h")
- **Until mode** (`--until DATETIME`): Execute until a specific date/time (ISO 8601 format)
- **Progress bar**: Nicely formatted progress bar display for all limit modes
- **Cross-platform**: Works on Windows, macOS, and Linux
- **Modern diff highlighting**: Uses console colors for clear visual diff output

## Installation

### From Cargo (crates.io)
```shell
cargo install watch-rs
watchr --help
```

### From Source
```shell
git clone https://github.com/DrEsteban/watch-rust
cd watch-rust
cargo install --path .
```

## Usage

### Basic Usage
```shell
# Watch a command every 2 seconds (default)
watchr "ls -la"

# Watch with custom interval
watchr -n 5 "df -h"

# Watch with diff highlighting
watchr -d "date"
```

### Execution Limits
```shell
# Execute exactly 10 times
watchr --count 10 "echo hello"

# Execute for 5 minutes
watchr --duration 5m "date"

# Execute for 1 hour and 30 minutes
watchr --duration 1h30m "uptime"

# Execute until a specific time
watchr --until "2024-12-31T23:59:59" "date"
```

### Advanced Options
```shell
# Highlight differences with permanent mode (shows all changes since start)
watchr --differences-permanent "cat /proc/meminfo"

# Exit when output changes
watchr -g "cat /var/log/syslog | tail -1"

# Exit when output is stable for 5 cycles
watchr -q 5 "date +%S"

# Beep on error and exit
watchr -b -e "some-command"

# Precise timing mode
watchr -p -n 1 "date +%N"

# No title, no wrap
watchr -t -w "ps aux"
```

## Duration Format

The `--duration` option accepts flexible time formats:
- `30s` or `30` - 30 seconds
- `5m` - 5 minutes
- `2h` - 2 hours
- `1d` - 1 day
- `1h30m` - 1 hour and 30 minutes
- `2h45m30s` - 2 hours, 45 minutes, and 30 seconds

## Exit Codes

- `0` - Success (normal exit or limit reached)
- `1` - Command execution error
- Other - Propagated from the watched command

## Key Bindings

While watching:
- `q` - Quit
- `Ctrl+C` - Quit

## Requirements

- Rust 1.70 or later
- Works on Windows, macOS, and Linux

## License

See [LICENSE](LICENSE) file.

## Author

DrEsteban

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
