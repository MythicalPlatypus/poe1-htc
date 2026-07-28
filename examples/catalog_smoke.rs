//! Release-mode smoke probe for the read-only A3 catalog API.

use std::env;

use anyhow::{ensure, Context, Result};
use poe1_htc::app::{CleanBaseAffixQuery, OptimizerService};
use poe1_htc::data::loader::load_all_with_provenance;

const DEFAULT_BASE_ID: &str = "Metadata/Items/Armours/BodyArmours/BodyStr15";

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let data_dir = args.next().unwrap_or_else(|| "data".to_string());
    let base_id = args.next().unwrap_or_else(|| DEFAULT_BASE_ID.to_string());
    let item_level = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()
        .context("item level must be an integer")?
        .unwrap_or(86);

    let service = OptimizerService::from_loaded(load_all_with_provenance(&data_dir)?);
    let catalog = service.compatible_clean_base_affixes(&CleanBaseAffixQuery {
        base_id,
        item_level,
    })?;

    ensure!(
        !catalog.prefixes.is_empty() && !catalog.suffixes.is_empty(),
        "expected the selected base to have at least one compatible prefix and suffix"
    );

    println!(
        "{} ({}) at item level {}: {} prefixes, {} suffixes",
        catalog.base.name,
        catalog.base.id,
        catalog.item_level,
        catalog.prefixes.len(),
        catalog.suffixes.len()
    );
    println!(
        "stable ID bounds: {} .. {} / {} .. {}",
        catalog.prefixes.first().expect("checked nonempty").id,
        catalog.prefixes.last().expect("checked nonempty").id,
        catalog.suffixes.first().expect("checked nonempty").id,
        catalog.suffixes.last().expect("checked nonempty").id
    );
    println!(
        "data fingerprint: {}",
        service.data_provenance().fingerprint()
    );
    Ok(())
}
