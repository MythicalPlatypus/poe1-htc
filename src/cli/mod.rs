use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "poe1_htc", about = "Path of Exile 1 crafting path optimizer")]
pub struct Args {
    /// Path to the RePoE data directory
    #[arg(long, default_value = "data")]
    pub data_dir: String,

    /// Base item name to craft on (e.g. "Astral Plate")
    #[arg(long)]
    pub base_item: Option<String>,

    /// Beam width for the search (higher = more thorough, slower)
    #[arg(long, default_value_t = 100)]
    pub beam_width: usize,

    /// Maximum number of crafting steps to simulate
    #[arg(long, default_value_t = 20)]
    pub max_steps: usize,
}

pub fn run(args: Args) -> Result<()> {
    println!("POE1 HTC — Crafting Path Optimizer");
    println!("Data dir : {}", args.data_dir);
    println!("Base item: {:?}", args.base_item);
    println!("Beam width: {}, Max steps: {}", args.beam_width, args.max_steps);

    let db = crate::data::loader::load_all(&args.data_dir)?;
    println!("Loaded {} mods from RePoE", db.mods.len());

    Ok(())
}
