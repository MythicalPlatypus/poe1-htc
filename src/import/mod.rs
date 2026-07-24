//! Path of Exile clipboard-item parsing and deterministic RePoE modifier
//! matching.
//!
//! Clipboard text contains rendered modifier lines, while RePoE stores display
//! templates (usually rendered ranges, and occasionally `{0}` placeholders).
//! Import therefore matches text structure first, tests displayed numbers
//! against candidate stat ranges, and only then emits concrete RePoE stat
//! values. No crafting state is mutated here; [`crate::goal::build_imported_state`]
//! owns state validation and construction.

use std::collections::{BTreeSet, HashSet};

use anyhow::{anyhow, bail, Context, Result};

use crate::data::{
    base_items::BaseItem,
    mods::{Domain, GenerationType, Mod, ModStat},
    GameData,
};
use crate::item::state::Rarity;

/// Import behavior selected by the caller.
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    /// Item level to use when abbreviated text omits `Item Level:`.
    pub fallback_item_level: Option<u32>,
    /// Reject unresolved/ambiguous explicit modifier text instead of skipping
    /// it with a warning.
    pub strict: bool,
}

/// Parsed clipboard item before validation into [`crate::item::ItemState`].
#[derive(Debug, Clone)]
pub struct ImportedItem {
    pub item_name: Option<String>,
    pub base_name: String,
    pub rarity: Rarity,
    pub item_level: Option<u32>,
    pub quality: Option<u32>,
    pub sockets: Option<String>,
    pub displayed_energy_shield: Option<i32>,
    pub explicit_mods: Vec<ImportedModifier>,
    pub implicit_mods: Vec<ImportedModifier>,
    pub enchantments: Vec<ImportedModifier>,
    pub corrupted: bool,
    pub mirrored: bool,
    pub warnings: Vec<ImportWarning>,
}

/// One resolved RePoE modifier and its raw stat values.
#[derive(Debug, Clone)]
pub struct ImportedModifier {
    pub mod_id: String,
    pub values: Vec<i32>,
    pub fractured: bool,
    pub crafted: bool,
    pub displayed_lines: Vec<String>,
}

/// Non-fatal import diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Explicit,
    Implicit,
    Enchant,
}

#[derive(Debug, Clone)]
struct DisplayLine {
    text: String,
    source_line: usize,
    kind: SectionKind,
    fractured: bool,
    crafted: bool,
    run: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    imported: ImportedModifier,
    generation_type: GenerationType,
    groups: Vec<String>,
    score: i64,
    consumed: usize,
}

#[derive(Debug, Clone, Default)]
struct ExplicitLayout {
    prefixes: usize,
    suffixes: usize,
    crafted: usize,
    groups: HashSet<String>,
}

#[derive(Debug, Clone)]
struct Solution {
    candidates: Vec<Candidate>,
    score: i64,
}

#[derive(Debug, Clone)]
enum NumberSpec {
    Placeholder(usize),
    Range(f64, f64),
    Fixed(f64),
}

#[derive(Debug, Clone)]
enum PatternPart {
    Literal(String),
    Number(NumberSpec),
}

#[derive(Debug, Clone)]
struct CapturedNumber {
    spec: NumberSpec,
    observed: f64,
    line: String,
}

#[derive(Debug, Clone)]
struct ValueMapping {
    values: Vec<i32>,
    matched_slots: usize,
}

/// Parse clipboard text and resolve all displayed modifiers against RePoE.
pub fn import_item_text(text: &str, db: &GameData, options: ImportOptions) -> Result<ImportedItem> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let raw_lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim().trim_start_matches('\u{feff}').to_string())
        .collect();
    if raw_lines.iter().all(|line| line.is_empty()) {
        bail!("clipboard item text is empty");
    }

    let (rarity_index, rarity) = parse_rarity(&raw_lines)?;
    let mut warnings = Vec::new();
    let resolved = resolve_base(&raw_lines, rarity_index, &rarity, db, &mut warnings)?;
    let base_index = resolved.line_index;
    let base_id = resolved.id;
    let base = resolved.base;

    let item_name = if resolved.inferred_from_name {
        Some(raw_lines[base_index].clone())
    } else {
        raw_lines[rarity_index + 1..base_index]
            .iter()
            .rfind(is_header_name_line)
            .cloned()
    };

    let mut item_level = None;
    let mut quality = None;
    let mut sockets = None;
    let mut displayed_energy_shield = None;
    let mut corrupted = false;
    let mut mirrored = false;
    let mut modifier_lines = Vec::new();
    let mut next_run = 0usize;
    let mut previous_kind = None;
    let mut block = 0usize;
    let mut previous_block = 0usize;

    for (index, raw) in raw_lines.iter().enumerate() {
        if raw.is_empty() {
            continue;
        }
        if is_separator(raw) {
            block += 1;
            previous_kind = None;
            continue;
        }
        if index == rarity_index || index == base_index {
            continue;
        }
        if item_name
            .as_ref()
            .is_some_and(|name| index > rarity_index && index < base_index && raw == name)
        {
            continue;
        }

        let lower = raw.to_ascii_lowercase();
        if lower == "corrupted" {
            corrupted = true;
            continue;
        }
        if lower == "mirrored" {
            mirrored = true;
            continue;
        }
        if lower == "unidentified" {
            bail!(
                "line {}: unidentified items cannot be imported because their hidden modifiers are unknowable",
                index + 1
            );
        }
        if lower == "unmodifiable" || lower.starts_with("modifiable only with ") {
            bail!(
                "line {}: restricted-modification item status '{}' is not modeled",
                index + 1,
                raw
            );
        }
        if let Some(value) = property_value(raw, "item level") {
            let parsed = parse_first_i32(value)
                .with_context(|| format!("line {}: malformed Item Level", index + 1))?;
            if !(1..=100).contains(&parsed) {
                bail!(
                    "line {}: Item Level {parsed} must be between 1 and 100",
                    index + 1
                );
            }
            item_level = Some(parsed as u32);
            continue;
        }
        if let Some((value, catalyst_kind)) = quality_property_value(raw) {
            let parsed = parse_first_i32(value)
                .with_context(|| format!("line {}: malformed Quality", index + 1))?;
            if parsed < 0 {
                bail!("line {}: Quality cannot be negative", index + 1);
            }
            quality = Some(parsed as u32);
            if let Some(kind) = catalyst_kind {
                bail!(
                    "line {}: catalyst quality '{kind}' is not supported because its modifier-magnitude scaling cannot be imported safely",
                    index + 1
                );
            }
            continue;
        }
        if let Some(value) = property_value(raw, "sockets") {
            let value = strip_trailing_display_annotation(value).trim();
            if value.is_empty() {
                bail!("line {}: Sockets value is empty", index + 1);
            }
            sockets = Some(value.to_string());
            continue;
        }
        if let Some(value) = property_value(raw, "energy shield") {
            displayed_energy_shield = Some(
                parse_first_i32(value)
                    .with_context(|| format!("line {}: malformed Energy Shield", index + 1))?,
            );
            continue;
        }
        if lower.starts_with("rarity:") || lower.starts_with("item class:") {
            continue;
        }
        if raw.starts_with('{') && raw.ends_with('}') {
            warnings.push(ImportWarning {
                code: "unsupported_metadata".to_string(),
                message: format!(
                    "line {}: ignored advanced modifier metadata '{}'",
                    index + 1,
                    raw
                ),
            });
            continue;
        }
        if is_known_metadata(raw, base) {
            if should_warn_metadata(raw) {
                warnings.push(ImportWarning {
                    code: "unsupported_metadata".to_string(),
                    message: format!("line {}: ignored metadata '{}'", index + 1, raw),
                });
            }
            continue;
        }
        let (display, kind, fractured, crafted) = parse_annotation(raw)?;
        if is_reminder_line(&display) {
            warnings.push(ImportWarning {
                code: "unsupported_metadata".to_string(),
                message: format!("line {}: ignored reminder text '{}'", index + 1, raw),
            });
            continue;
        }
        if kind != SectionKind::Explicit && (fractured || crafted) {
            bail!(
                "line {}: implicit/enchantment text cannot be fractured or crafted",
                index + 1
            );
        }
        if previous_kind != Some(kind) || previous_block != block {
            next_run += 1;
        }
        previous_kind = Some(kind);
        previous_block = block;
        modifier_lines.push(DisplayLine {
            text: display,
            source_line: index + 1,
            kind,
            fractured,
            crafted,
            run: next_run,
        });
    }

    if item_level.is_none() {
        if let Some(fallback) = options.fallback_item_level {
            if !(1..=100).contains(&fallback) {
                bail!("fallback item level {fallback} must be between 1 and 100");
            }
            item_level = Some(fallback);
            warnings.push(ImportWarning {
                code: "missing_item_level".to_string(),
                message: format!(
                    "clipboard text omitted Item Level; using goal item level {fallback}"
                ),
            });
        } else {
            warnings.push(ImportWarning {
                code: "missing_item_level".to_string(),
                message: "clipboard text omitted Item Level and no fallback was supplied"
                    .to_string(),
            });
        }
    }

    let enchant_lines = lines_of_kind(&modifier_lines, SectionKind::Enchant);
    let mut enchantments = match_sequence(
        &enchant_lines,
        SectionKind::Enchant,
        rarity.clone(),
        base,
        item_level,
        db,
        0,
        0,
        options.strict,
        &mut warnings,
    )?;
    let (defence_effect, resistance_effect) = magnitude_effects(&enchantments, db);

    let implicit_lines = lines_of_kind(&modifier_lines, SectionKind::Implicit);
    let implicit_mods = match_sequence(
        &implicit_lines,
        SectionKind::Implicit,
        rarity.clone(),
        base,
        item_level,
        db,
        0,
        0,
        options.strict,
        &mut warnings,
    )?;

    let explicit_lines = lines_of_kind(&modifier_lines, SectionKind::Explicit);
    let explicit_mods = match_sequence(
        &explicit_lines,
        SectionKind::Explicit,
        rarity.clone(),
        base,
        item_level,
        db,
        defence_effect,
        resistance_effect,
        options.strict,
        &mut warnings,
    )?;

    // Preserve display order within each special-state collection.
    enchantments.shrink_to_fit();

    let same_name_count = db
        .base_items
        .values()
        .filter(|candidate| case_insensitive_eq(&candidate.name, &base.name))
        .count();
    // The public contract has no base_id field. Keep the human display name in
    // the common case, but retain metadata identity when names are duplicated
    // so CLI re-resolution cannot silently select a different base.
    let base_name = if same_name_count > 1 {
        base_id.to_string()
    } else {
        base.name.clone()
    };

    Ok(ImportedItem {
        item_name,
        base_name,
        rarity,
        item_level,
        quality,
        sockets,
        displayed_energy_shield,
        explicit_mods,
        implicit_mods,
        enchantments,
        corrupted,
        mirrored,
        warnings,
    })
}

struct ResolvedBase<'a> {
    id: &'a str,
    base: &'a BaseItem,
    line_index: usize,
    inferred_from_name: bool,
}

fn parse_rarity(lines: &[String]) -> Result<(usize, Rarity)> {
    for (index, line) in lines.iter().enumerate() {
        let Some(value) = property_value(line, "rarity") else {
            continue;
        };
        let rarity = match value.trim().to_ascii_lowercase().as_str() {
            "normal" => Rarity::Normal,
            "magic" => Rarity::Magic,
            "rare" => Rarity::Rare,
            "unique" => Rarity::Unique,
            other => bail!(
                "line {}: unsupported rarity '{other}' (expected Normal, Magic, Rare, or Unique)",
                index + 1
            ),
        };
        return Ok((index, rarity));
    }
    bail!("clipboard item text is missing a 'Rarity:' header")
}

fn resolve_base<'a>(
    lines: &[String],
    rarity_index: usize,
    rarity: &Rarity,
    db: &'a GameData,
    warnings: &mut Vec<ImportWarning>,
) -> Result<ResolvedBase<'a>> {
    for (line_index, line) in lines.iter().enumerate().skip(rarity_index + 1) {
        if let Some((id, base)) = db.base_items.get_key_value(line) {
            let duplicates = duplicate_base_ids(&base.name, db);
            if duplicates.len() > 1 {
                warnings.push(ImportWarning {
                    code: "duplicate_base_name".to_string(),
                    message: format!(
                        "base '{}' has duplicate display names; preserving exact metadata ID '{}'",
                        base.name, id
                    ),
                });
            }
            return Ok(ResolvedBase {
                id,
                base,
                line_index,
                inferred_from_name: false,
            });
        }
    }

    for (line_index, line) in lines.iter().enumerate().skip(rarity_index + 1) {
        let mut matches: Vec<(&String, &BaseItem)> = db
            .base_items
            .iter()
            .filter(|(_, base)| case_insensitive_eq(&base.name, line))
            .collect();
        matches.sort_by_key(|(id, _)| id.to_string());
        if matches.is_empty() {
            continue;
        }
        if matches.len() > 1 {
            let implicit_matches = bases_matching_displayed_implicit(&matches, lines, db);
            if implicit_matches.len() == 1 {
                let (id, base) = implicit_matches[0];
                warnings.push(ImportWarning {
                    code: "base_implicit_disambiguation".to_string(),
                    message: format!(
                        "resolved duplicate base name '{}' to metadata ID '{}' from its displayed implicit",
                        line, id
                    ),
                });
                return Ok(ResolvedBase {
                    id,
                    base,
                    line_index,
                    inferred_from_name: false,
                });
            }
            warnings.push(ImportWarning {
                code: "ambiguous_base_name".to_string(),
                message: format!(
                    "{} bases share display name '{}'; using metadata ID '{}' deterministically",
                    matches.len(),
                    line,
                    matches[0].0
                ),
            });
        }
        return Ok(ResolvedBase {
            id: matches[0].0,
            base: matches[0].1,
            line_index,
            inferred_from_name: false,
        });
    }

    // Quality and synthesis decorate the displayed type line even though
    // RePoE stores the undecorated base name. Decorations can be combined.
    for (line_index, line) in lines.iter().enumerate().skip(rarity_index + 1) {
        let (base_name, decorated, synthesised) = strip_base_decorations(line);
        if !decorated {
            continue;
        }
        let mut matches: Vec<(&String, &BaseItem)> = db
            .base_items
            .iter()
            .filter(|(_, base)| case_insensitive_eq(&base.name, base_name))
            .collect();
        matches.sort_by_key(|(id, _)| id.to_string());
        if matches.is_empty() {
            continue;
        }
        if matches.len() > 1 {
            let implicit_matches = bases_matching_displayed_implicit(&matches, lines, db);
            if implicit_matches.len() == 1 {
                let (id, base) = implicit_matches[0];
                warnings.push(ImportWarning {
                    code: "base_implicit_disambiguation".to_string(),
                    message: format!(
                        "resolved duplicate decorated base name '{}' to metadata ID '{}' from its displayed implicit",
                        line, id
                    ),
                });
                if synthesised {
                    warnings.push(ImportWarning {
                        code: "unsupported_metadata".to_string(),
                        message: format!(
                            "resolved synthesised base '{}' but synthesis status itself is not modeled",
                            line
                        ),
                    });
                }
                return Ok(ResolvedBase {
                    id,
                    base,
                    line_index,
                    inferred_from_name: false,
                });
            }
            warnings.push(ImportWarning {
                code: "ambiguous_base_name".to_string(),
                message: format!(
                    "{} bases share synthesised display name '{}'; using metadata ID '{}' deterministically",
                    matches.len(),
                    base_name,
                    matches[0].0
                ),
            });
        }
        if synthesised {
            warnings.push(ImportWarning {
                code: "unsupported_metadata".to_string(),
                message: format!(
                    "resolved synthesised base '{}' but synthesis status itself is not modeled",
                    line
                ),
            });
        }
        return Ok(ResolvedBase {
            id: matches[0].0,
            base: matches[0].1,
            line_index,
            inferred_from_name: false,
        });
    }

    // Magic-item clipboard text commonly contains only the affixed item name
    // (for example, "Chemist's Granite Flask of the Deer") rather than a
    // separate base-name line. Infer an embedded base by the longest
    // case-insensitive display-name match, then preserve metadata identity if
    // that display name is duplicated.
    if matches!(rarity, Rarity::Magic) {
        for (line_index, line) in lines.iter().enumerate().skip(rarity_index + 1) {
            if is_separator(line) || line.contains(':') {
                break;
            }
            let line_lower = line.to_lowercase();
            let mut matches: Vec<(&String, &BaseItem)> = db
                .base_items
                .iter()
                .filter(|(_, base)| {
                    let name = base.name.to_lowercase();
                    !name.is_empty() && contains_words(&line_lower, &name)
                })
                .collect();
            matches.sort_by(|(left_id, left), (right_id, right)| {
                right
                    .name
                    .len()
                    .cmp(&left.name.len())
                    .then_with(|| left_id.cmp(right_id))
            });
            let Some((id, base)) = matches.first().copied() else {
                continue;
            };
            let longest = base.name.len();
            let same_length = matches
                .iter()
                .take_while(|(_, candidate)| candidate.name.len() == longest)
                .count();
            if same_length > 1
                && !matches[..same_length]
                    .iter()
                    .all(|(_, candidate)| case_insensitive_eq(&candidate.name, &base.name))
            {
                continue;
            }
            warnings.push(ImportWarning {
                code: "inferred_base_name".to_string(),
                message: format!(
                    "inferred base '{}' ({id}) from magic item name '{}'",
                    base.name, line
                ),
            });
            return Ok(ResolvedBase {
                id,
                base,
                line_index,
                inferred_from_name: true,
            });
        }
    }

    bail!(
        "could not resolve a base item after the Rarity header (tried exact metadata IDs and case-insensitive display names)"
    )
}

fn strip_base_decorations(mut line: &str) -> (&str, bool, bool) {
    let mut decorated = false;
    let mut synthesised = false;
    loop {
        let lower = line.to_ascii_lowercase();
        let mut stripped = false;
        for (prefix, synthesis) in [
            ("superior ", false),
            ("synthesised ", true),
            ("synthesized ", true),
        ] {
            if lower.starts_with(prefix) {
                line = &line[prefix.len()..];
                decorated = true;
                synthesised |= synthesis;
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    (line, decorated, synthesised)
}

fn contains_words(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, matched)| {
        let end = start + matched.len();
        let left_ok = start == 0
            || haystack[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric());
        let right_ok = end == haystack.len()
            || haystack[end..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_alphanumeric());
        left_ok && right_ok
    })
}

fn case_insensitive_eq(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

fn duplicate_base_ids(name: &str, db: &GameData) -> Vec<String> {
    let mut ids: Vec<String> = db
        .base_items
        .iter()
        .filter(|(_, base)| case_insensitive_eq(&base.name, name))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

fn bases_matching_displayed_implicit<'a>(
    matches: &[(&'a String, &'a BaseItem)],
    lines: &[String],
    db: &'a GameData,
) -> Vec<(&'a String, &'a BaseItem)> {
    let observed: Vec<String> = lines
        .iter()
        .filter_map(|line| parse_annotation(line).ok())
        .filter(|(_, kind, _, _)| *kind == SectionKind::Implicit)
        .map(|(text, _, _, _)| text)
        .collect();
    matches
        .iter()
        .copied()
        .filter(|(_, base)| {
            base.implicits.iter().any(|id| {
                let Some(text) = db
                    .mods
                    .get(id)
                    .and_then(|modifier| modifier.text.as_deref())
                else {
                    return false;
                };
                let templates: Vec<&str> = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect();
                !templates.is_empty()
                    && observed.windows(templates.len()).any(|window| {
                        templates.iter().zip(window).all(|(template, displayed)| {
                            capture_line(template, displayed).is_some()
                        })
                    })
            })
        })
        .collect()
}

fn lines_of_kind(lines: &[DisplayLine], kind: SectionKind) -> Vec<DisplayLine> {
    lines
        .iter()
        .filter(|line| line.kind == kind)
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn match_sequence(
    lines: &[DisplayLine],
    kind: SectionKind,
    rarity: Rarity,
    base: &BaseItem,
    item_level: Option<u32>,
    db: &GameData,
    defence_effect: i32,
    resistance_effect: i32,
    strict: bool,
    warnings: &mut Vec<ImportWarning>,
) -> Result<Vec<ImportedModifier>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_lists: Vec<Vec<Candidate>> = (0..lines.len())
        .map(|index| {
            candidates_at(
                lines,
                index,
                kind,
                base,
                item_level,
                db,
                defence_effect,
                resistance_effect,
            )
        })
        .collect();
    let layout = ExplicitLayout::default();
    let mut solutions = Vec::new();
    let mut furthest = 0usize;
    solve_sequence(
        lines,
        0,
        kind,
        rarity.clone(),
        &candidate_lists,
        &layout,
        &mut Vec::new(),
        0,
        &mut solutions,
        &mut furthest,
    );

    if solutions.is_empty() {
        let failure_index = furthest.min(lines.len() - 1);
        let failure = &lines[failure_index];
        let diagnostics = nearby_candidates(failure, kind, base, item_level, db);
        let suffix = if diagnostics.is_empty() {
            String::new()
        } else {
            format!("; candidate diagnostics: {}", diagnostics.join(", "))
        };
        if strict {
            bail!(
                "line {}: could not resolve {} modifier text '{}'{}",
                failure.source_line,
                section_label(kind),
                failure.text,
                suffix
            );
        }
        warnings.push(ImportWarning {
            code: "unresolved_modifier".to_string(),
            message: format!(
                "line {}: skipped unresolved {} modifier '{}'{}",
                failure.source_line,
                section_label(kind),
                failure.text,
                suffix
            ),
        });
        let mut remaining = lines.to_vec();
        remaining.remove(failure_index);
        return match_sequence(
            &remaining,
            kind,
            rarity,
            base,
            item_level,
            db,
            defence_effect,
            resistance_effect,
            false,
            warnings,
        );
    }

    let segmentations: BTreeSet<String> = solutions
        .iter()
        .map(|solution| {
            solution
                .candidates
                .iter()
                .map(|candidate| candidate.consumed.to_string())
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect();
    if kind == SectionKind::Explicit && segmentations.len() > 1 {
        let solution_refs: Vec<&Solution> = solutions.iter().collect();
        let candidates = ambiguous_ids(&solution_refs);
        let message = format!(
            "genuinely ambiguous explicit modifier segmentation beginning at line {} (layouts: {}; candidates: {})",
            lines[0].source_line,
            segmentations.into_iter().collect::<Vec<_>>().join(", "),
            candidates.into_iter().take(16).collect::<Vec<_>>().join(", ")
        );
        if strict {
            bail!("{message}");
        }
        warnings.push(ImportWarning {
            code: "ambiguous_modifier_segmentation".to_string(),
            message,
        });
    }

    solutions.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| solution_key(left).cmp(&solution_key(right)))
    });
    let best_score = solutions[0].score;
    let mut best: Vec<&Solution> = solutions
        .iter()
        .filter(|solution| solution.score == best_score)
        .collect();
    best.sort_by_key(|solution| solution_key(solution));
    best.dedup_by_key(|solution| solution_key(solution));

    if let Some(candidate) = best[0].candidates.iter().find(|candidate| {
        candidate.imported.values.is_empty()
            && db
                .mods
                .get(&candidate.imported.mod_id)
                .is_some_and(|modifier| !modifier.stats.is_empty())
    }) {
        bail!(
            "line {}: '{}' has variable hidden stat values that clipboard text does not expose; importing it would fabricate rolls",
            lines[0].source_line,
            candidate.imported.mod_id
        );
    }

    if best.len() > 1 {
        let ids = ambiguous_ids(&best);
        if kind == SectionKind::Explicit && strict {
            bail!(
                "genuinely ambiguous explicit modifier text beginning at line {}; candidates: {}",
                lines[0].source_line,
                ids.join(", ")
            );
        }
        warnings.push(ImportWarning {
            code: "ambiguous_special_modifier".to_string(),
            message: format!(
                "ambiguous {} modifier text beginning at line {}; using '{}' deterministically from candidates: {}",
                section_label(kind),
                lines[0].source_line,
                best[0]
                    .candidates
                    .first()
                    .map(|candidate| candidate.imported.mod_id.as_str())
                    .unwrap_or("<none>"),
                ids.join(", ")
            ),
        });
    }

    Ok(best[0]
        .candidates
        .iter()
        .map(|candidate| candidate.imported.clone())
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn solve_sequence(
    lines: &[DisplayLine],
    index: usize,
    kind: SectionKind,
    rarity: Rarity,
    candidate_lists: &[Vec<Candidate>],
    layout: &ExplicitLayout,
    selected: &mut Vec<Candidate>,
    score: i64,
    output: &mut Vec<Solution>,
    furthest: &mut usize,
) {
    const MAX_SOLUTIONS: usize = 4_096;
    if output.len() >= MAX_SOLUTIONS {
        return;
    }
    *furthest = (*furthest).max(index);
    if index == lines.len() {
        let candidate_count = i64::try_from(selected.len()).unwrap_or(1).max(1);
        output.push(Solution {
            candidates: selected.clone(),
            // Candidate evidence should not become stronger merely because a
            // multi-line modifier was split into several one-line modifiers.
            score: score.saturating_mul(1_024) / candidate_count,
        });
        return;
    }

    for candidate in &candidate_lists[index] {
        let Some(next_layout) = place_candidate(layout, candidate, kind, &rarity) else {
            continue;
        };
        selected.push(candidate.clone());
        solve_sequence(
            lines,
            index + candidate.consumed,
            kind,
            rarity.clone(),
            candidate_lists,
            &next_layout,
            selected,
            score + candidate.score,
            output,
            furthest,
        );
        selected.pop();
    }
}

fn place_candidate(
    layout: &ExplicitLayout,
    candidate: &Candidate,
    kind: SectionKind,
    rarity: &Rarity,
) -> Option<ExplicitLayout> {
    if kind != SectionKind::Explicit {
        return Some(layout.clone());
    }

    let max = match rarity {
        Rarity::Normal => 0,
        Rarity::Magic => 1,
        Rarity::Rare | Rarity::Unique => 3,
    };
    let mut next = layout.clone();
    if candidate
        .groups
        .iter()
        .any(|group| next.groups.contains(group))
    {
        return None;
    }
    match candidate.generation_type {
        GenerationType::Prefix if next.prefixes < max => next.prefixes += 1,
        GenerationType::Suffix if next.suffixes < max => next.suffixes += 1,
        _ => return None,
    }
    if candidate.imported.crafted {
        next.crafted += 1;
        if next.crafted > 1 {
            return None;
        }
    }
    next.groups.extend(candidate.groups.iter().cloned());
    Some(next)
}

#[allow(clippy::too_many_arguments)]
fn candidates_at(
    lines: &[DisplayLine],
    index: usize,
    kind: SectionKind,
    base: &BaseItem,
    item_level: Option<u32>,
    db: &GameData,
    defence_effect: i32,
    resistance_effect: i32,
) -> Vec<Candidate> {
    let first = &lines[index];
    let mut ids: Vec<&String> = db.mods.keys().collect();
    ids.sort();
    let base_tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    let mut result = Vec::new();

    for id in ids {
        let modifier = &db.mods[id];
        let Some(text) = modifier.text.as_deref() else {
            continue;
        };
        if !section_compatible(modifier, kind, first.crafted, base) {
            continue;
        }
        if item_level.is_some_and(|level| modifier.required_level > level) {
            continue;
        }
        let templates: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if templates.is_empty() || index + templates.len() > lines.len() {
            continue;
        }
        let displayed = &lines[index..index + templates.len()];
        if displayed.iter().any(|line| line.run != first.run) {
            continue;
        }
        if displayed
            .iter()
            .any(|line| line.crafted != first.crafted || line.fractured != first.fractured)
        {
            continue;
        }
        if first.fractured && first.crafted {
            continue;
        }

        let mut captures = Vec::new();
        let mut structural_match = true;
        for (template, observed) in templates.iter().zip(displayed) {
            match capture_line(template, &observed.text) {
                Some(mut line_captures) => captures.append(&mut line_captures),
                None => {
                    structural_match = false;
                    break;
                }
            }
        }
        if !structural_match {
            continue;
        }

        let effect = if kind == SectionKind::Explicit {
            modifier_magnitude_effect(modifier, defence_effect, resistance_effect)
        } else {
            0
        };
        let spawn_weight = modifier.spawn_weight_for_tags(&base_tags);
        let base_specific_spawn_weight = modifier
            .spawn_weights
            .iter()
            .find(|weight| weight.tag != "default" && base_tags.contains(&weight.tag.as_str()))
            .map_or(0, |weight| weight.weight);
        let base_implicit = base.implicits.iter().any(|implicit| implicit == id);
        let mut mappings = map_values(modifier, &captures, effect);
        if mappings.is_empty()
            && captures.is_empty()
            && modifier.stats.iter().any(|stat| stat.min != stat.max)
            && (spawn_weight > 0 || base_implicit)
        {
            // The display identifies a real candidate, but none of its
            // variable stats is printed (for example modern curse-on-hit
            // implicit levels). Keep it as a sentinel so a legacy fixed-stat
            // lookalike cannot be silently selected instead.
            mappings.push(ValueMapping {
                values: Vec::new(),
                matched_slots: 0,
            });
        }
        if mappings.is_empty() {
            continue;
        }

        for mapping in mappings {
            let mut score = i64::from(modifier.required_level);
            score += i64::try_from(mapping.matched_slots).unwrap_or(0) * 5;
            score += i64::try_from(templates.len()).unwrap_or(1) * 2_000;
            if mapping.values.is_empty() && !modifier.stats.is_empty() {
                score += 10_000;
            }
            if base_specific_spawn_weight > 0 {
                score += 1_000;
            } else if spawn_weight > 0 {
                score += 100;
            }
            if base_implicit {
                score += 5_000;
            }
            if kind == SectionKind::Explicit
                && spawn_weight == 0
                && !first.fractured
                && !first.crafted
            {
                // Zero-weight legacy/transferred mods remain candidates, but a
                // currently valid base/domain match must outrank them.
                score -= 1_000;
            }
            score += domain_score(&modifier.domain, kind, base);
            score += generation_score(modifier, kind);
            score -= i64::try_from(modifier.stats.len().saturating_sub(mapping.matched_slots))
                .unwrap_or(0);

            result.push(Candidate {
                imported: ImportedModifier {
                    mod_id: id.clone(),
                    values: mapping.values,
                    fractured: first.fractured,
                    crafted: first.crafted,
                    displayed_lines: displayed.iter().map(|line| line.text.clone()).collect(),
                },
                generation_type: modifier.generation_type.clone(),
                groups: modifier.groups.clone(),
                score,
                consumed: templates.len(),
            });
        }
    }

    result.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.imported.mod_id.cmp(&right.imported.mod_id))
            .then_with(|| left.imported.values.cmp(&right.imported.values))
    });
    result
}

fn section_compatible(modifier: &Mod, kind: SectionKind, crafted: bool, base: &BaseItem) -> bool {
    match kind {
        SectionKind::Explicit => {
            if !matches!(
                modifier.generation_type,
                GenerationType::Prefix | GenerationType::Suffix
            ) {
                return false;
            }
            if crafted {
                return modifier.domain == Domain::Crafted;
            }
            if modifier.domain == Domain::Crafted {
                return false;
            }
            if !explicit_domain_compatible(&modifier.domain, base) {
                return false;
            }
            !matches!(
                modifier.domain,
                Domain::Monster
                    | Domain::Area
                    | Domain::Map
                    | Domain::Stance
                    | Domain::Tempest
                    | Domain::Leaguestone
                    | Domain::Watchstone
                    | Domain::HeistArea
                    | Domain::SentinelTag
                    | Domain::MemoryLine
                    | Domain::Expedition
                    | Domain::Necropolis
            )
        }
        SectionKind::Implicit => !matches!(
            modifier.generation_type,
            GenerationType::Prefix | GenerationType::Suffix | GenerationType::Enchantment
        ),
        SectionKind::Enchant => !matches!(
            modifier.generation_type,
            GenerationType::Prefix | GenerationType::Suffix
        ),
    }
}

fn explicit_domain_compatible(domain: &Domain, base: &BaseItem) -> bool {
    match domain {
        Domain::Chest | Domain::Synthesis => false,
        Domain::Abyss => base.item_class == "AbyssJewel",
        Domain::Affliction => matches!(base.item_class.as_str(), "AnimalCharm" | "Jewel"),
        Domain::Sanctum => base.item_class.contains("Relic"),
        Domain::HeistEquipment => base.item_class.starts_with("HeistEquipment"),
        Domain::Trinket => base.tags.iter().any(|tag| tag.contains("trinket")),
        _ => true,
    }
}

fn domain_score(domain: &Domain, kind: SectionKind, base: &BaseItem) -> i64 {
    let specialized_unknown_domain = matches!(
        base.item_class.as_str(),
        "AbyssJewel" | "Tincture" | "AnimalCharm"
    ) || base.item_class.starts_with("HeistEquipment");
    match (kind, domain) {
        (SectionKind::Explicit, Domain::Item) if specialized_unknown_domain => 50,
        (SectionKind::Explicit, Domain::Item | Domain::Crafted) => 500,
        (SectionKind::Explicit, Domain::Delve | Domain::Veiled) => 400,
        (SectionKind::Explicit, Domain::Abyss) if base.item_class == "AbyssJewel" => 500,
        (SectionKind::Explicit, Domain::Affliction)
            if matches!(base.item_class.as_str(), "AnimalCharm" | "Jewel") =>
        {
            500
        }
        (SectionKind::Explicit, Domain::Sanctum) if base.item_class.contains("Relic") => 500,
        (SectionKind::Explicit, Domain::HeistEquipment)
            if base.item_class.starts_with("HeistEquipment") =>
        {
            500
        }
        (SectionKind::Explicit, Domain::Trinket)
            if base.tags.iter().any(|tag| tag.contains("trinket")) =>
        {
            500
        }
        (SectionKind::Explicit, Domain::Unknown) if specialized_unknown_domain => 500,
        (SectionKind::Explicit, Domain::Unknown) => 0,
        (_, Domain::Item) => 20,
        _ => 0,
    }
}

fn generation_score(modifier: &Mod, kind: SectionKind) -> i64 {
    match kind {
        SectionKind::Explicit => 100,
        SectionKind::Implicit => match modifier.generation_type {
            GenerationType::ExarchImplicit | GenerationType::EaterImplicit => 120,
            GenerationType::Corrupted => 100,
            GenerationType::Unique => 80,
            _ => 40,
        },
        SectionKind::Enchant => {
            let looks_like_enchant = modifier
                .groups
                .iter()
                .any(|group| group.to_ascii_lowercase().contains("enchant"))
                || modifier.name.to_ascii_lowercase().contains("enchant");
            if modifier.generation_type == GenerationType::Enchantment {
                140
            } else if looks_like_enchant {
                130
            } else {
                30
            }
        }
    }
}

fn capture_line(template: &str, observed: &str) -> Option<Vec<CapturedNumber>> {
    let template = normalize_spaces(template);
    let observed = normalize_spaces(observed);
    let parts = pattern_parts(&template)?;
    let mut cursor = 0usize;
    let mut captures = Vec::new();
    for (part_index, part) in parts.iter().enumerate() {
        match part {
            PatternPart::Literal(literal) => {
                let end = cursor.checked_add(literal.len())?;
                if end > observed.len()
                    || !observed.is_char_boundary(cursor)
                    || !observed.is_char_boundary(end)
                    || !observed[cursor..end].eq_ignore_ascii_case(literal.as_str())
                {
                    return None;
                }
                cursor = end;
            }
            PatternPart::Number(spec) => {
                let (value, end) = parse_number_at(&observed, cursor)?;
                cursor = end;
                // Ctrl+Alt+C can append the unmodified roll range immediately
                // after a displayed number, e.g. `+112(91-100)`.
                if observed[cursor..].starts_with('(') {
                    if let Some(close) = observed[cursor..].find(')') {
                        let inside = &observed[cursor + 1..cursor + close];
                        if parse_range(inside).is_some() {
                            cursor += close + 1;
                        }
                    }
                }
                captures.push(CapturedNumber {
                    spec: spec.clone(),
                    observed: value,
                    line: template.clone(),
                });
                if part_index + 1 == parts.len() && cursor != observed.len() {
                    return None;
                }
            }
        }
    }
    (cursor == observed.len()).then_some(captures)
}

fn pattern_parts(template: &str) -> Option<Vec<PatternPart>> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut index = 0usize;
    while index < template.len() {
        let character = template[index..].chars().next()?;
        if character == '{' {
            if let Some(close_rel) = template[index + 1..].find('}') {
                let close = index + 1 + close_rel;
                if let Ok(placeholder) = template[index + 1..close].parse::<usize>() {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(PatternPart::Number(NumberSpec::Placeholder(placeholder)));
                    index = close + 1;
                    continue;
                }
            }
        }
        if character == '(' {
            if let Some(close_rel) = template[index + 1..].find(')') {
                let close = index + 1 + close_rel;
                if let Some((mut min, mut max)) = parse_range(&template[index + 1..close]) {
                    if literal.ends_with('-') {
                        literal.pop();
                        (min, max) = (-max, -min);
                    }
                    flush_literal(&mut parts, &mut literal);
                    parts.push(PatternPart::Number(NumberSpec::Range(min, max)));
                    index = close + 1;
                    continue;
                }
            }
        }
        let previous_is_alphabetic = template[..index]
            .chars()
            .next_back()
            .is_some_and(char::is_alphabetic);
        if character.is_ascii_digit() && !previous_is_alphabetic {
            let mut number_start = index;
            if matches!(literal.chars().next_back(), Some('+' | '-')) {
                literal.pop();
                number_start -= 1;
            }
            let end = number_end(template, index);
            let value = parse_f64(&template[number_start..end])?;
            flush_literal(&mut parts, &mut literal);
            parts.push(PatternPart::Number(NumberSpec::Fixed(value)));
            index = end;
            continue;
        }
        literal.push(character);
        index += character.len_utf8();
    }
    flush_literal(&mut parts, &mut literal);
    Some(parts)
}

fn flush_literal(parts: &mut Vec<PatternPart>, literal: &mut String) {
    if !literal.is_empty() {
        parts.push(PatternPart::Literal(std::mem::take(literal)));
    }
}

fn number_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len()
        && (bytes[index].is_ascii_digit() || matches!(bytes[index], b'.' | b','))
    {
        index += 1;
    }
    index
}

fn parse_number_at(text: &str, start: usize) -> Option<(f64, usize)> {
    let bytes = text.as_bytes();
    let mut index = start;
    if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
        index += 1;
    }
    let digits_start = index;
    while index < bytes.len()
        && (bytes[index].is_ascii_digit() || matches!(bytes[index], b'.' | b','))
    {
        index += 1;
    }
    if index == digits_start {
        return None;
    }
    Some((parse_f64(&text[start..index])?, index))
}

fn parse_range(text: &str) -> Option<(f64, f64)> {
    let bytes = text.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] != b'-' {
            continue;
        }
        let left = parse_f64(&text[..index]);
        let right = parse_f64(&text[index + 1..]);
        if let (Some(left), Some(right)) = (left, right) {
            return Some((left, right));
        }
    }
    None
}

fn parse_f64(text: &str) -> Option<f64> {
    text.replace(',', "").parse().ok()
}

fn map_values(modifier: &Mod, captures: &[CapturedNumber], effect: i32) -> Vec<ValueMapping> {
    let mut output = Vec::new();
    let mut values = vec![None; modifier.stats.len()];
    assign_values(
        modifier,
        captures,
        0,
        0,
        effect,
        &mut values,
        0,
        &mut output,
    );
    output.sort_by(|left, right| left.values.cmp(&right.values));
    output.dedup_by(|left, right| left.values == right.values);
    output
}

#[allow(clippy::too_many_arguments)]
fn assign_values(
    modifier: &Mod,
    captures: &[CapturedNumber],
    capture_index: usize,
    next_stat: usize,
    effect: i32,
    values: &mut [Option<i32>],
    matched_slots: usize,
    output: &mut Vec<ValueMapping>,
) {
    const MAX_MAPPINGS: usize = 64;
    if output.len() >= MAX_MAPPINGS {
        return;
    }
    if capture_index == captures.len() {
        let mut filled = Vec::new();
        let mut complete = true;
        for (index, stat) in modifier.stats.iter().enumerate() {
            if values[index].is_none() {
                let value = if stat.min == stat.max {
                    Some(stat.min)
                } else {
                    // One displayed roll can feed several identical-range
                    // RePoE stats (hybrid resistances and hidden
                    // display-nothing companions).
                    (0..index).rev().find_map(|previous| {
                        let previous_stat = &modifier.stats[previous];
                        (previous_stat.min == stat.min && previous_stat.max == stat.max)
                            .then_some(values[previous])
                            .flatten()
                    })
                };
                let Some(value) = value else {
                    complete = false;
                    break;
                };
                values[index] = Some(value);
                filled.push(index);
            }
        }
        if complete {
            output.push(ValueMapping {
                values: values.iter().filter_map(|value| *value).collect(),
                matched_slots,
            });
        }
        for index in filled {
            values[index] = None;
        }
        return;
    }

    let capture = &captures[capture_index];
    if let NumberSpec::Placeholder(stat_index) = capture.spec {
        if stat_index >= modifier.stats.len() || stat_index < next_stat {
            return;
        }
        if !fill_fixed_between(modifier, values, next_stat, stat_index) {
            return;
        }
        for raw in matching_raw_values(&modifier.stats[stat_index], capture, effect, false) {
            values[stat_index] = Some(raw);
            assign_values(
                modifier,
                captures,
                capture_index + 1,
                stat_index + 1,
                effect,
                values,
                matched_slots + 1,
                output,
            );
            values[stat_index] = None;
        }
        clear_fixed_between(values, next_stat, stat_index);
        return;
    }

    let mut mapped = false;
    for stat_index in next_stat..modifier.stats.len() {
        if modifier.stats[next_stat..stat_index]
            .iter()
            .any(|stat| stat.min != stat.max)
        {
            break;
        }
        if !spec_matches_stat(&capture.spec, &modifier.stats[stat_index], &capture.line) {
            continue;
        }
        if !fill_fixed_between(modifier, values, next_stat, stat_index) {
            continue;
        }
        for raw in matching_raw_values(&modifier.stats[stat_index], capture, effect, true) {
            mapped = true;
            values[stat_index] = Some(raw);
            assign_values(
                modifier,
                captures,
                capture_index + 1,
                stat_index + 1,
                effect,
                values,
                matched_slots + 1,
                output,
            );
            values[stat_index] = None;
        }
        clear_fixed_between(values, next_stat, stat_index);
    }

    // A number that cannot represent a stat (e.g. "every 4 seconds") is a
    // static literal. It must remain exactly equal to its template value.
    if !mapped && capture_matches_static(capture) {
        assign_values(
            modifier,
            captures,
            capture_index + 1,
            next_stat,
            effect,
            values,
            matched_slots,
            output,
        );
    }
}

fn fill_fixed_between(
    modifier: &Mod,
    values: &mut [Option<i32>],
    start: usize,
    end: usize,
) -> bool {
    for (stat, value) in modifier.stats[start..end]
        .iter()
        .zip(&mut values[start..end])
    {
        if stat.min != stat.max {
            return false;
        }
        *value = Some(stat.min);
    }
    true
}

fn clear_fixed_between(values: &mut [Option<i32>], start: usize, end: usize) {
    for value in &mut values[start..end] {
        *value = None;
    }
}

fn spec_matches_stat(spec: &NumberSpec, stat: &ModStat, line: &str) -> bool {
    stat_display_scale(spec, stat, line).is_some()
}

fn stat_display_scale(spec: &NumberSpec, stat: &ModStat, line: &str) -> Option<f64> {
    const KNOWN_SCALES: [f64; 14] = [
        -2.0, -1.0, 0.000_1, 0.001, 0.01, 0.1, 0.2, 0.5, 1.0, 1.5, 2.0, 10.0, 100.0, 1_000.0,
    ];
    let mut min = raw_display_value(stat.min, stat, line);
    let mut max = raw_display_value(stat.max, stat, line);
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    match spec {
        NumberSpec::Placeholder(_) => Some(1.0),
        NumberSpec::Range(left, right) => {
            let target_min = left.min(*right);
            let target_max = left.max(*right);
            KNOWN_SCALES.into_iter().find(|scale| {
                let scaled_min = (min * scale).min(max * scale);
                let scaled_max = (min * scale).max(max * scale);
                approx_eq(scaled_min, target_min) && approx_eq(scaled_max, target_max)
            })
        }
        NumberSpec::Fixed(value) => {
            if stat.min != stat.max {
                return None;
            }
            if approx_eq(min, 0.0) {
                if approx_eq(*value, 0.0) {
                    Some(1.0)
                } else {
                    None
                }
            } else {
                KNOWN_SCALES
                    .into_iter()
                    .find(|scale| approx_eq(min * scale, *value))
            }
        }
    }
}

fn display_scale_for_capture(capture: &CapturedNumber, stat: &ModStat) -> Option<f64> {
    match capture.spec {
        NumberSpec::Placeholder(_) => Some(1.0),
        _ => stat_display_scale(&capture.spec, stat, &capture.line),
    }
}

fn matching_raw_values(
    stat: &ModStat,
    capture: &CapturedNumber,
    effect: i32,
    require_spec: bool,
) -> Vec<i32> {
    if require_spec && !spec_matches_stat(&capture.spec, stat, &capture.line) {
        return Vec::new();
    }
    let Some(display_scale) = display_scale_for_capture(capture, stat) else {
        return Vec::new();
    };
    let span = i64::from(stat.max) - i64::from(stat.min);
    let candidates: Vec<i32> = if span <= 200_000 {
        (stat.min..=stat.max).collect()
    } else {
        let factor = f64::from(100 + effect) / 100.0;
        let per_second = stat.id.contains("per_minute")
            && capture.line.to_ascii_lowercase().contains("per second");
        let unit = if per_second { 60.0 } else { 1.0 };
        let estimate = if factor.abs() < f64::EPSILON || display_scale.abs() < f64::EPSILON {
            f64::from(stat.min)
        } else {
            capture.observed * unit / factor / display_scale
        };
        let center = estimate.round() as i64;
        let mut candidates = BTreeSet::new();
        candidates.insert(stat.min);
        candidates.insert(stat.max);
        for raw in center - 128..=center + 128 {
            if raw >= i64::from(stat.min) && raw <= i64::from(stat.max) {
                candidates.insert(raw as i32);
            }
        }
        candidates.into_iter().collect()
    };
    let mut matches: Vec<(i32, f64)> = candidates
        .into_iter()
        .filter_map(|raw| {
            let base = raw_display_value(raw, stat, &capture.line) * display_scale;
            displayed_matches(base, effect, capture.observed).then(|| {
                let scaled = base * f64::from(100 + effect) / 100.0;
                (raw, (scaled - capture.observed).abs())
            })
        })
        .collect();
    let best_error = matches
        .iter()
        .map(|(_, error)| *error)
        .fold(f64::INFINITY, f64::min);
    matches.retain(|(_, error)| (*error - best_error).abs() < 0.000_001);
    matches.into_iter().map(|(raw, _)| raw).collect()
}

fn raw_display_value(raw: i32, stat: &ModStat, line: &str) -> f64 {
    let lower = line.to_ascii_lowercase();
    let mut value = f64::from(raw);
    if stat.id.contains("per_minute") && lower.contains("per second") {
        value /= 60.0;
    }
    if [" reduced ", " less ", " slower "]
        .iter()
        .any(|word| lower.contains(word))
        || [" reduced", " less", " slower"]
            .iter()
            .any(|word| lower.ends_with(word))
    {
        value = value.abs();
    }
    value
}

fn displayed_matches(base: f64, effect: i32, observed: f64) -> bool {
    if effect == 0 {
        return approx_eq(base, observed);
    }
    let scaled = base * f64::from(100 + effect) / 100.0;
    if observed.fract().abs() < 0.000_001 {
        (scaled.trunc() - observed).abs() < 0.000_001
    } else {
        ((scaled * 10.0).round() / 10.0 - observed).abs() < 0.000_001
    }
}

fn capture_matches_static(capture: &CapturedNumber) -> bool {
    match capture.spec {
        NumberSpec::Fixed(value) => approx_eq(value, capture.observed),
        NumberSpec::Range(min, max) => {
            capture.observed >= min.min(max) && capture.observed <= min.max(max)
        }
        NumberSpec::Placeholder(_) => false,
    }
}

fn approx_eq(left: f64, right: f64) -> bool {
    (left - right).abs() < 0.051
}

fn modifier_magnitude_effect(modifier: &Mod, defence_effect: i32, resistance_effect: i32) -> i32 {
    let mut effect = 0;
    if modifier.tags.iter().any(|tag| tag == "defences") {
        effect += defence_effect;
    }
    if modifier.tags.iter().any(|tag| tag == "resistance") {
        effect += resistance_effect;
    }
    effect
}

fn magnitude_effects(enchantments: &[ImportedModifier], db: &GameData) -> (i32, i32) {
    let mut defence = 0;
    let mut resistance = 0;
    for imported in enchantments {
        let Some(modifier) = db.mods.get(&imported.mod_id) else {
            continue;
        };
        for (stat, value) in modifier.stats.iter().zip(&imported.values) {
            match stat.id.as_str() {
                "heist_enchantment_defence_mod_effect_+%" => defence += *value,
                "heist_enchantment_resistance_mod_effect_+%" => resistance += *value,
                _ => {}
            }
        }
    }
    (defence, resistance)
}

fn nearby_candidates(
    line: &DisplayLine,
    kind: SectionKind,
    base: &BaseItem,
    item_level: Option<u32>,
    db: &GameData,
) -> Vec<String> {
    let observed_skeleton = text_skeleton(&line.text);
    let base_tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    let mut candidates: Vec<(i64, String)> = db
        .mods
        .iter()
        .filter_map(|(id, modifier)| {
            let text = modifier.text.as_deref()?;
            if !section_compatible(modifier, kind, line.crafted, base)
                || item_level.is_some_and(|level| modifier.required_level > level)
            {
                return None;
            }
            let first = text.lines().next().unwrap_or_default();
            let mut score = common_prefix_len(&observed_skeleton, &text_skeleton(first)) as i64;
            if modifier.spawn_weight_for_tags(&base_tags) > 0 {
                score += 20;
            }
            (score > 2).then(|| (score, id.clone()))
        })
        .collect();
    candidates.sort_by(|(left_score, left_id), (right_score, right_id)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_id.cmp(right_id))
    });
    candidates.into_iter().take(8).map(|(_, id)| id).collect()
}

fn text_skeleton(text: &str) -> String {
    pattern_parts(&normalize_spaces(text))
        .unwrap_or_default()
        .into_iter()
        .map(|part| match part {
            PatternPart::Literal(literal) => literal.to_ascii_lowercase(),
            PatternPart::Number(_) => "#".to_string(),
        })
        .collect()
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn solution_key(solution: &Solution) -> String {
    solution
        .candidates
        .iter()
        .map(|candidate| {
            format!(
                "{}:{:?}:{}",
                candidate.imported.mod_id, candidate.imported.values, candidate.consumed
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn ambiguous_ids(solutions: &[&Solution]) -> Vec<String> {
    let mut ids = BTreeSet::new();
    let max_len = solutions
        .iter()
        .map(|solution| solution.candidates.len())
        .max()
        .unwrap_or(0);
    for index in 0..max_len {
        let variants: BTreeSet<String> = solutions
            .iter()
            .filter_map(|solution| solution.candidates.get(index))
            .map(|candidate| {
                format!(
                    "{} {:?}",
                    candidate.imported.mod_id, candidate.imported.values
                )
            })
            .collect();
        if variants.len() > 1 {
            ids.extend(variants);
        }
    }
    ids.into_iter().collect()
}

fn parse_annotation(raw: &str) -> Result<(String, SectionKind, bool, bool)> {
    let mut text = raw.trim().to_string();
    let mut kind = SectionKind::Explicit;
    let mut fractured = false;
    let mut crafted = false;
    loop {
        let lower = text.to_ascii_lowercase();
        let Some((suffix, action)) = [
            (" (fractured)", 0u8),
            (" (crafted)", 1u8),
            (" (implicit)", 2u8),
            (" (enchant)", 3u8),
            (" — unscalable value", 4u8),
            (" - unscalable value", 4u8),
        ]
        .into_iter()
        .find(|(suffix, _)| lower.ends_with(suffix)) else {
            break;
        };
        text.truncate(text.len() - suffix.len());
        text = text.trim_end().to_string();
        match action {
            0 => fractured = true,
            1 => crafted = true,
            2 => kind = SectionKind::Implicit,
            3 => kind = SectionKind::Enchant,
            4 => {}
            _ => unreachable!(),
        }
    }
    if fractured && crafted {
        bail!("modifier line '{raw}' cannot be both fractured and crafted");
    }
    if text.is_empty() {
        bail!("modifier annotation '{raw}' has no displayed text");
    }
    Ok((text, kind, fractured, crafted))
}

fn normalize_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn property_value<'a>(line: &'a str, property: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case(property)
        .then_some(value.trim())
}

fn quality_property_value(line: &str) -> Option<(&str, Option<&str>)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if key.eq_ignore_ascii_case("quality") {
        return Some((value.trim(), None));
    }
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("quality (") && key.ends_with(')') {
        let kind = key["Quality (".len()..key.len() - 1].trim();
        return Some((value.trim(), Some(kind)));
    }
    None
}

fn parse_first_i32(text: &str) -> Result<i32> {
    let mut token = String::new();
    let mut started = false;
    for character in text.chars() {
        if character.is_ascii_digit() || (!started && matches!(character, '+' | '-')) {
            token.push(character);
            started = true;
        } else if character == ',' && started {
            continue;
        } else if started {
            break;
        }
    }
    if token.is_empty() || token == "+" || token == "-" {
        return Err(anyhow!("no integer value found in '{text}'"));
    }
    token
        .parse()
        .with_context(|| format!("invalid integer value '{token}'"))
}

fn strip_trailing_display_annotation(value: &str) -> &str {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    for annotation in [" (augmented)", " (unmet)"] {
        if lower.ends_with(annotation) {
            return &trimmed[..trimmed.len() - annotation.len()];
        }
    }
    trimmed
}

fn is_separator(line: &str) -> bool {
    line.len() >= 4 && line.chars().all(|character| character == '-')
}

fn is_header_name_line(line: &&String) -> bool {
    !line.is_empty()
        && !line.contains(':')
        && !is_separator(line)
        && !matches!(
            line.to_ascii_lowercase().as_str(),
            "corrupted" | "mirrored" | "unidentified"
        )
}

fn compact_label(text: &str) -> String {
    text.to_lowercase()
        .replace("handed", "hand")
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn is_known_metadata(line: &str, base: &BaseItem) -> bool {
    let lower = line.to_ascii_lowercase();
    let property = [
        "requirements:",
        "level:",
        "str:",
        "dex:",
        "int:",
        "armour:",
        "evasion rating:",
        "ward:",
        "chance to block:",
        "physical damage:",
        "elemental damage:",
        "chaos damage:",
        "critical strike chance:",
        "attacks per second:",
        "weapon range:",
        "radius:",
        "limited to:",
        "map tier:",
        "atlas region:",
        "item quantity:",
        "item rarity:",
        "monster pack size:",
        "more scarabs:",
        "more maps:",
        "more currency:",
        "monster level:",
        "note:",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));
    let base_class =
        !line.contains(':') && compact_label(line) == compact_label(base.item_class.as_str());
    let flask_property = (lower.starts_with("lasts ") && lower.ends_with(" seconds"))
        || (lower.starts_with("recovers ")
            && lower.contains(" over ")
            && lower.ends_with(" seconds"))
        || (lower.starts_with("consumes ") && lower.contains(" charges on use"))
        || (lower.starts_with("currently has ") && lower.ends_with(" charges"))
        || lower.starts_with("right click to drink.");
    let jewel_or_map_footer = lower.starts_with("place into an allocated ")
        || lower.starts_with("place into an abyssal socket")
        || lower.starts_with("travel to this map ")
        || lower.starts_with("modifiable only with ");
    let tincture_property = (lower.starts_with("inflicts mana burn every ")
        && lower.ends_with(" seconds"))
        || (lower.contains(" second cooldown when deactivated"))
        || lower.starts_with("right click to activate.");
    let heist_metadata = base.item_class.starts_with("HeistEquipment")
        && (matches!(
            lower.as_str(),
            "any heist member can equip this item."
                | "this item can be equipped by:"
                | "can only be equipped to heist members."
        ) || (lower.starts_with("level ") && lower.contains(" in "))
            || [
                "karst,", "tibbs,", "isla,", "tullina,", "nenet,", "vinderi,", "gianna,", "huck,",
                "niles,",
            ]
            .iter()
            .any(|name| lower.starts_with(name)));
    property
        || base_class
        || flask_property
        || jewel_or_map_footer
        || tincture_property
        || heist_metadata
        || matches!(
            lower.as_str(),
            "unidentified"
                | "split"
                | "abyss"
                | "unmodifiable"
                | "fractured item"
                | "synthesised item"
                | "searing exarch item"
                | "eater of worlds item"
                | "crucible item"
                | "shaper item"
                | "elder item"
                | "crusader item"
                | "hunter item"
                | "redeemer item"
                | "warlord item"
        )
}

fn is_reminder_line(line: &str) -> bool {
    let text = line.trim();
    text.strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .and_then(|inner| inner.chars().next())
        .is_some_and(char::is_alphabetic)
}

fn should_warn_metadata(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    !matches!(lower.as_str(), "requirements:" | "unidentified")
        && !lower.starts_with("level:")
        && !lower.starts_with("str:")
        && !lower.starts_with("dex:")
        && !lower.starts_with("int:")
}

fn section_label(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Explicit => "explicit",
        SectionKind::Implicit => "implicit",
        SectionKind::Enchant => "enchantment",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rendered_ranges_and_placeholders() {
        let captures = capture_line(
            "+(91-100) to maximum Energy Shield",
            "+97 to maximum Energy Shield",
        )
        .expect("rendered range should match");
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].observed, 97.0);

        let captures = capture_line("{0}% increased Armour\n", "25% increased Armour")
            .expect("placeholder should match");
        assert!(matches!(captures[0].spec, NumberSpec::Placeholder(0)));

        let captures = capture_line("-3 to maximum Sockets", "-3 to maximum Sockets")
            .expect("fixed negative values should retain their sign");
        assert!(matches!(
            captures[0].spec,
            NumberSpec::Fixed(value) if value == -3.0
        ));
        assert_eq!(captures[0].observed, -3.0);
    }

    #[test]
    fn accepts_advanced_roll_range_after_value() {
        let captures = capture_line(
            "+(91-100) to maximum Energy Shield",
            "+112(91-100) to maximum Energy Shield",
        )
        .expect("advanced roll-range decoration should be ignored");
        assert_eq!(captures[0].observed, 112.0);
    }

    #[test]
    fn recognizes_common_repoe_display_scales() {
        let permyriad = ModStat {
            id: "life_leech_permyriad".to_string(),
            min: 20,
            max: 40,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(0.2, 0.4),
                &permyriad,
                "(0.2-0.4)% of Damage Leeched as Life",
            ),
            Some(0.01)
        );

        let weapon_range = ModStat {
            id: "weapon_range".to_string(),
            min: 1,
            max: 1,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Fixed(0.1),
                &weapon_range,
                "+0.1 metres to Weapon Range",
            ),
            Some(0.1)
        );

        let suppression = ModStat {
            id: "spell_suppression".to_string(),
            min: 10,
            max: 20,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(15.0, 30.0),
                &suppression,
                "(15-30)% chance to Suppress Spell Damage",
            ),
            Some(1.5)
        );

        let rounded_per_minute = ModStat {
            id: "mana_regeneration_rate_per_minute".to_string(),
            min: 196,
            max: 240,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(3.3, 4.0),
                &rounded_per_minute,
                "Regenerate (3.3-4) Mana per second",
            ),
            Some(1.0)
        );

        let fortification = ModStat {
            id: "max_fortification_+1_per_5".to_string(),
            min: 15,
            max: 25,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(3.0, 5.0),
                &fortification,
                "+(3-5) to maximum Fortification",
            ),
            Some(0.2)
        );

        let halved = ModStat {
            id: "heist_contract_lockdown_timer_+%_halved".to_string(),
            min: 12,
            max: 14,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(6.0, 7.0),
                &halved,
                "(6-7)% increased time before Lockdown",
            ),
            Some(0.5)
        );

        let doubled_negative = ModStat {
            id: "mana_reservation_efficiency_-2%_per_1".to_string(),
            min: -5,
            max: -4,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(10.0, 8.0),
                &doubled_negative,
                "(10-8)% increased Mana Reservation Efficiency",
            ),
            Some(-2.0)
        );

        let negative_encoding = ModStat {
            id: "action_speed_-%".to_string(),
            min: -6,
            max: -4,
        };
        assert_eq!(
            stat_display_scale(
                &NumberSpec::Range(4.0, 6.0),
                &negative_encoding,
                "(4-6)% increased Action Speed",
            ),
            Some(-1.0)
        );
    }

    #[test]
    fn annotation_parsing_is_case_insensitive() {
        let (text, kind, fractured, crafted) =
            parse_annotation("+1 to Spectres (FrAcTuReD)").unwrap();
        assert_eq!(text, "+1 to Spectres");
        assert_eq!(kind, SectionKind::Explicit);
        assert!(fractured);
        assert!(!crafted);

        let (_, kind, _, _) = parse_annotation("Effect (EnChAnT)").unwrap();
        assert_eq!(kind, SectionKind::Enchant);

        let (text, _, _, crafted) =
            parse_annotation("Inflicts Decay (crafted) — Unscalable Value").unwrap();
        assert_eq!(text, "Inflicts Decay");
        assert!(crafted);
        assert!(is_reminder_line(
            "(Leeched Life is recovered over time. Multiple Leeches can occur simultaneously)"
        ));
        assert!(!is_reminder_line("(15-18)% increased Strength"));
    }

    #[test]
    fn per_minute_display_conversion_uses_per_second_text() {
        let stat = ModStat {
            id: "energy_shield_regeneration_rate_per_minute".to_string(),
            min: 12_000,
            max: 12_000,
        };
        assert_eq!(
            raw_display_value(12_000, &stat, "Regenerate 200 Energy Shield per second"),
            200.0
        );
        assert!(displayed_matches(200.0, 12, 224.0));

        let rounded = ModStat {
            id: "base_life_regeneration_rate_per_minute".to_string(),
            min: 126,
            max: 480,
        };
        let capture = CapturedNumber {
            spec: NumberSpec::Range(2.1, 8.0),
            observed: 3.0,
            line: "Regenerate (2.1-8) Life per second".to_string(),
        };
        assert_eq!(matching_raw_values(&rounded, &capture, 0, true), vec![180]);

        let less = ModStat {
            id: "local_flask_duration_+%_final".to_string(),
            min: -49,
            max: -45,
        };
        assert_eq!(raw_display_value(-49, &less, "49% less Duration"), 49.0);
        let slower = ModStat {
            id: "wither_expire_speed_+%".to_string(),
            min: -12,
            max: -10,
        };
        assert_eq!(
            raw_display_value(-12, &slower, "Withered expires 12% slower"),
            12.0
        );
    }

    #[test]
    fn malformed_integer_has_context() {
        assert!(parse_first_i32("(augmented)").is_err());
        assert_eq!(parse_first_i32("+1,200%").unwrap(), 1200);
    }
}
