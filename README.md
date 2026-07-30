# watch-rust (`watch-rs`)

`watchr` is a cross-platform Rust implementation of Linux's `watch`. It repeatedly runs a command in a full-screen terminal display and highlights, follows, or stops on output changes.

## Features

### Linux `watch` compatibility

- Interval control with `-n, --interval`, including `WATCH_INTERVAL`
- Beep on error with `-b, --beep`
- Safe ANSI SGR color handling with `-c, --color` and `-C, --no-color`
- Character-level change highlighting with `-d, --differences[=permanent]`
- Command-status propagation with `-e, --errexit`
- Scrolling output with `-f, --follow`
- Exit on changed or unchanged visible output with `-g, --chgexit` and `-q, --equexit`
- Precise scheduling with `-p, --precise`
- Resize, title, wrapping, and direct-exec controls
- Plain-text screenshots with `s` and `-s, --shotsdir`
- Immediate reruns with the spacebar

### Extended limits

- `--count N` runs exactly N times
- `--duration DURATION` runs for a bounded duration
- `--until DATETIME` runs until a local or RFC 3339 timestamp
- Bounded runs show progress and print a final summary

## Installation

From crates.io:

```shell
cargo install watch-rs
watchr --help
```

From source:

```shell
git clone https://github.com/DrEsteban/watch-rust
cd watch-rust
cargo install --path . --locked
```

## Usage

```shell
# Run every two seconds
watchr ls -la

# Run every half-second
watchr -n 0.5 df -h

# Use a shell pipeline
watchr 'cat /proc/meminfo | head'

# Bypass the shell and preserve argument boundaries
watchr --exec printf '%s\n' 'hello world'
```

Option parsing stops at the command. Arguments after it, including values beginning with `-`, are passed to the watched command.

### Differences

```shell
# Highlight changes since the previous update
watchr -d date

# Keep every highlight since the first update
watchr -d1 date
watchr --differences=permanent date
```

`--differences-permanent` remains available as a descriptive alias.

### Output and exit conditions

```shell
# Scroll each update instead of clearing
watchr --follow journalctl -n 5

# Exit when visible output changes
watchr --chgexit cat status.txt

# Exit after five unchanged visible updates
watchr --equexit 5 cat status.txt

# Freeze on failure and return the command's status after a key press
watchr --errexit health-check
```

`--follow` cannot be combined with differences, `--chgexit`, or `--equexit` because those modes compare full-screen cells.

### Execution limits

```shell
watchr --count 10 echo hello
watchr --duration 5m date
watchr --until 2030-12-31T23:59:59 uptime
watchr --until 2030-12-31T23:59:59Z uptime
```

Duration components may use days, hours, minutes, and seconds:

- `30` or `30s`
- `5m`
- `2h`
- `1d`
- `1h30m`
- `2h45m30s`

### Interval behavior

Intervals are clamped to the Linux `watch` range of 0.1 seconds through 2,678,400 seconds (31 days). Both `1.5` and `1,5` are accepted.

Set a default with:

```shell
export WATCH_INTERVAL=5
watchr date
```

An explicit `--interval` overrides the environment value.

## Key controls

- `q` or `Ctrl+C`: quit
- Spacebar: run the command immediately
- `s`: save the current visible frame as `watch_YYYYMMDD-HHMMSS`

Use `--shotsdir DIR` to select the screenshot directory.

## Exit status

- `0`: normal exit, a reached limit, or a change condition
- `1`: invalid input, terminal, or command-management failure
- With `--errexit`: the command's non-zero exit code
- With `--errexit` on Unix signal termination: `128 + signal`

## Requirements

- Rust 1.85 or later
- An interactive terminal on Windows, macOS, or Linux

## Release packaging

Release CI builds and uploads a Windows MSI, then updates the existing `DrEsteban.watch-rust` WinGet manifest. The initial WinGet manifest must be submitted once with Winget-Create before automated updates can run.

## License

See [LICENSE](LICENSE).
