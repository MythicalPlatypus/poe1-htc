# FAQ & Troubleshooting

## Setup

**`cargo: command not found` / `'cargo' is not recognized`**
Rust isn't installed or the terminal predates the install. Install from
<https://rustup.rs>, then open a **new** terminal.

**The build fails with linker errors on Windows.**
Rustup needs the Visual Studio C++ Build Tools. Re-run the rustup installer
and accept its prompt to install them, or grab "Build Tools for Visual
Studio" from Microsoft and select the C++ workload.

**`Failed to read ...` / errors about `data/mods.json`**
The RePoE data files aren't downloaded (they are deliberately not in the
repository). Follow [Getting Started §3](getting-started.md#3-download-the-game-data).
If your data lives elsewhere, point at it with `--data-dir <path>`.

**The data check reports far fewer mods than ~39,000.**
A download was truncated or an old export is in place. Re-download; each
file should be several megabytes.

## Running

**It's very slow.**
Use `cargo run --release --`; a debug build is many times slower. Then
lower `beam_width` / `max_steps` while iterating on a goal, and raise them
for the final answer.

**Every run gives a slightly different plan.**
Rolls are sampled, so unseeded runs differ. Set `--seed <N>` for
reproducible output, and see
[Understanding Results — stability](understanding-results.md#is-the-recommendation-stable)
for how to judge whether a plan is robust.

**`No crafting path found — no method was applicable to the starting item.`**
The starting item accepts none of the available methods. Typical causes:
the item is corrupted or mirrored, every relevant modifier pool is empty, or
none of the configured special methods can fire. Unique starts are rejected
during validation rather than reaching this message.

**Why does it recommend Chaos-spamming instead of something clever?**
With the default method set, that often *is* the cheapest route. Give the
search your real options — bench crafts, essences, fossils, Harvest — via
`[[methods]]`, and give it real `[prices]`. The default orb set is always
enabled; `[[methods]]` adds options and is not an allowlist.

**Why is my expensive plan ranked below a cheap mediocre one (or vice versa)?**
Complete targets always sort ahead of incomplete ones. Within the same
completion class, `cost_weight` is the exchange rate between score and the
restart-adjusted expected cost. See
[Writing Goals — choosing cost_weight](writing-goals.md#choosing-cost_weight).
The built-in default is `0.0` (cost ignored) — set it in every goal.

## Goal files

**`unknown field` errors when loading my goal.**
Field names are strictly validated — check spelling against
[Writing Goals](writing-goals.md). This is deliberate: a typo'd field would
otherwise silently change the search.

**`[[wants]] ...: mod/group/stat not found in mods.json`**
The identifier doesn't exist in the loaded data. Copy IDs from a report
(run once with `--item-file` on a similar item), from `data/mods.json`
itself, or from poedb. Remember `group` wants a conflict group
(`IncreasedLife`), `mod_id` wants an exact tier (`IncreasedLife12` in the
current T1 body-armour life example).

**How do I require T1 specifically?**
Either `mod_id = "<the T1 id>"`, or `group` + `stat` + `min_value` at the
T1 threshold — see `goals/mirror_tier_chest.toml` for a worked example.

**Can I start from a Magic item, or with influences?**
Yes: `rarity = "magic"` with up to two `[[item.mods]]`, and
`influences = ["hunter"]` etc. Influences gate conqueror mod pools and
block Harvest augment, exactly as in game.

## Importing items

**Errors or odd results importing a paste** — see the dedicated
[error table](importing-your-item.md#common-errors-and-fixes). The two
worth repeating:

- Stale `mods.json` is the most common cause of "could not resolve
  modifier text" — re-download after every patch.
- Explicit-mod import refuses to guess between identical-looking mods or
  invent hidden rolls. That's a feature; fall back to `[[item.mods]]` for
  those edge cases.
- Influence, Synthesised, and Split status lines currently produce
  `unsupported_metadata` and are not carried into crafting state. Do not use
  an imported plan for influenced or Synthesised items.

**My item's quality / sockets / total ES don't affect the plan.**
Correct — they're carried and displayed, but socket crafting, catalysts,
and derived defence totals are not modeled.

## Trust and limitations

**Should I follow the plan blindly?**
No — treat it as a well-costed suggestion. The search is heuristic, full
rerolls are sampled (50 samples), and recovery after a missed one-shot is
not modeled as a policy. The printed "expected" and "restart" numbers are
different scenarios, not guaranteed bounds. Check plan stability across seeds
before committing big currency, and read
[Understanding Results](understanding-results.md) for what each number
does and doesn't promise.

**What's not modeled at all?**
Metamods (e.g. "Prefixes Cannot Be Changed"), Veiled currency and
unveiling, Awakener's Orb, imprints, recombinators, Orb of Conflict,
catalysts/quality effects, socket crafting, Rog, and various league
systems. The full list lives in
[README — Known Limitations](../README.md#known-limitations). If a real
strategy depends on a metamod, this tool cannot plan it yet.

Clipboard import also does not preserve influence, Synthesised, or Split
status. Use TOML for influenced starts; Synthesised starts are unsupported.

**Are the prices live?**
No. Prices are whatever you put in `[prices]` — snapshots you control.
Update them for your league; the defaults are placeholders.

**Does it touch the game or my account?**
No. It reads RePoE JSON exports and text you paste. There is no game
integration, no trade API, no network access at runtime.

## New league checklist

1. Re-download the five data files ([Getting Started §3](getting-started.md#3-download-the-game-data)).
2. `git pull` and `cargo build --release` for the latest code.
3. Update `[prices]` in your goal files to early-league reality.
4. Expect brand-new league mechanics to be unsupported until RePoE exports
   them and the tool models them.

## Reporting bugs

This is a guild beta — reports are the point. When something looks wrong,
please include:

- The goal file, and the item paste if you used `--item-file`
- The exact command line **including `--seed`** (add one if you didn't)
- The full output
- What you expected instead (a wiki/poedb link helps)

Deterministic seeds make your run reproducible on someone else's machine,
which is usually the difference between a five-minute fix and a shrug.
