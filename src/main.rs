use anyhow::Result;
use clap::Parser;
use poe1_htc::cli::{run, Args};

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}
