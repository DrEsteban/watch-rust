use std::fmt::Debug;
use std::io::Result;

use clap::{command, crate_authors, Parser};
use watch_rs::watch;

#[derive(Parser, Debug)]
#[command(version, author = crate_authors!(), about, long_about = None)]
#[command(help_template("\
{before-help}{name} {version}
Author: {author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
"))]
struct Args {
    /// The interval to run the command, in seconds
    #[arg(name = "interval", short, long, value_name="sec", default_value = "5")]
    interval: u64,
    /// The command to run
    #[arg(name = "command", required = true)]
    command: String,
    /// Any number of arguments to pass to the `command`
    #[arg(name = "args", required = false)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    watch(args.command, args.args, args.interval)
}
