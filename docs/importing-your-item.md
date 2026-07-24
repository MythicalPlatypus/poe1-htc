# Importing Your Item

If the item you want to finish already exists, don't describe it by hand —
let the game do it. Path of Exile copies a full text description of any item
to the clipboard when you hover it and press `Ctrl+C`.

## The workflow

1. In game, hover the item and press `Ctrl+C`.
2. Paste into a text file (say `my_item.txt`) and save it.
3. Run with `--item-file`:

```bash
cargo run --release -- --goal my_goal.toml --item-file my_item.txt
```

On Linux/macOS you can pipe the clipboard straight in with `-`:

```bash
xclip -o | cargo run --release -- --goal my_goal.toml --item-file -
```

A goal file is **still required**: the import only replaces the goal's
`[item]` section. Your `[[wants]]`, `[[methods]]`, `[prices]`, and
`[search]` sections drive the search exactly as before. `--item-file`
cannot be combined with `--base-item`.

## What a paste looks like

```text
Item Class: Body Armours
Rarity: Rare
Corpse Shell
Astral Plate
--------
Armour: 711
--------
Requirements:
Level: 62
Str: 180
--------
Sockets: R-R-R
--------
Item Level: 86
--------
+12% to all Elemental Resistances (implicit)
--------
+180 to maximum Life (fractured)
+42 to Strength
```

The importer resolves the base (`Astral Plate`) and matches every modifier
line against the RePoE database to recover exact mod identities and rolls —
including the `(fractured)`, `(crafted)`, `(implicit)`, and `(enchant)`
annotations the game prints. Abbreviated, hand-written pastes work too; the
`--------` separators and stat blocks are optional.

## What gets modeled, what gets carried, what is display-only

- **Modeled crafting state** — rarity, item level, prefixes, suffixes,
  fractured and crafted mods, corrupted/mirrored status, and Eldritch
  implicits. These drive eligibility, capacity, group conflicts, and
  probabilities exactly as for goal-file items.
- **Preserved metadata** — generic implicits and enchantments are carried
  through every crafting step unchanged. They never consume affix slots or
  cause group conflicts, but they **can** satisfy `[[wants]]`.
- **Display-only metadata** — quality, sockets, and the displayed total
  Energy Shield are kept for reporting. Socket crafting and derived totals
  are not modeled, and the displayed total is not recomputed as mods change.

Existing mods are validated strictly (affix type, item level, roll ranges,
capacity, group conflicts, one crafted mod), but they do **not** need to be
currently rollable: fractured, Delve, unveiled, recombinated, or legacy mods
are accepted as existing state even at zero spawn weight. They still never
appear in new random rolls.

## The importer never guesses

Clipboard text is ambiguous more often than you'd think — two different mods
can print identical lines. The importer resolves what it can deterministically
(preferring mods that can actually spawn on your base at your item level) and
**errors instead of guessing** when candidates are genuinely
indistinguishable or when a mod has hidden rolls the text doesn't show.
Corrupted and mirrored items import fine as state, but note the optimizer
will refuse to craft on them for the obvious reason.

## Warnings you may see

Warnings are printed before the search and never silently change your item:

| Code | Meaning |
|---|---|
| `missing_item_level` | The paste had no `Item Level:` line; the goal file's `item_level` was used instead |
| `unsupported_metadata` | A property line (e.g. `Armour: 711`) was recognized and ignored — it isn't part of crafting state |
| `unresolved_modifier` | (non-strict contexts) A line couldn't be matched and was skipped |
| `inferred_base_name` / `duplicate_base_name` / `ambiguous_base_name` | The base line needed disambiguation; the report shows which base was chosen |
| `base_implicit_disambiguation` | Several bases share the name; your item's implicit picked the right one |

## Common errors and fixes

| Error mentions | What it means | Fix |
|---|---|---|
| `expected a 'Rarity:' header` | The file isn't a game paste (or lost its first lines) | Re-copy with `Ctrl+C`; keep the whole text |
| `unknown base item` | The base line didn't match `base_items.json` | Check for truncation; refresh your data files for the current patch |
| `could not resolve ... modifier text` | A line matched no known mod template | Usually stale data — re-download `mods.json`; the error lists nearby candidates |
| `genuinely ambiguous` | Two or more mods print this exact line and nothing distinguishes them | Rare; describe the item via `[[item.mods]]` with explicit `mod_id`s instead |
| `hidden stat values` | The mod has rolls the clipboard text doesn't display | The item can't be imported faithfully; use `[[item.mods]]` with known values |
| `unique items cannot be crafted on` | You imported a unique | The optimizer only crafts rare/magic/normal items |

## Why import instead of `[[item.mods]]`?

Both produce the same validated starting state. Import wins because the game
text carries exact rolls and annotations you would otherwise transcribe by
hand — and the importer tells you the RePoE mod IDs in its report, which you
can then use to write sharper `[[wants]]`.
