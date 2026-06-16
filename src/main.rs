mod data;
mod engine;
mod item;
mod currency;
mod search;
mod cli;

use anyhow::Result;
use cli::Args;
use clap::Parser;

fn main() -> Result<()> {
    let args = Args::parse();
    cli::run(args)
}
