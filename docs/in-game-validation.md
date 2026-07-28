# In-Game Validation

These checks compare live Path of Exile item text with the optimizer's modeled
starting state. Use disposable Standard-league items and cheap currency. Do not
test on anything you cannot replace.

The commands below are PowerShell commands run from the repository root. Keep
the complete game paste on the clipboard. The goal controls what the optimizer
searches for; `--item-file -` replaces only its starting item.

## Before testing

1. Build the release binary and confirm the automated suite:

   ```powershell
   cargo test
   cargo build --release
   ```

2. In Path of Exile, hover the item and press `Ctrl+C`.
3. Import the clipboard directly:

   ```powershell
   Get-Clipboard -Raw | cargo run --quiet --release -- --goal goals\import_validation_body_armour.toml --item-file - --expansion-limit 0 --seed 42
   ```

The zero-expansion limit makes the reported final item the unchanged imported
starting item, so its modifiers and properties can be compared directly.

## Test 1: Advanced modifier headers and preserved properties

Use the six-white-socket, 20%-quality Triumphant Lamellar represented by
`triumphant_lamellar.txt`, or copy that item again in game.

Expected:

- base ID is `Metadata/Items/Armours/BodyArmours/BodyStrDex17`;
- item level is 77, rarity is Rare, and there are four existing explicit mods;
- the four advanced `{ Prefix Modifier ... }` / `{ Suffix Modifier ... }`
  headers do not produce `unsupported_metadata` warnings;
- quality remains `+20%` and sockets remain `W-W-W-W-W-W`;
- warnings for the displayed `Armour:` and `Evasion Rating:` property lines are
  currently expected and harmless.

The checked-in capture can be tested without the game:

```powershell
cargo run --quiet --release -- --goal goals\import_validation_body_armour.toml --item-file triumphant_lamellar.txt --expansion-limit 0 --seed 42
```

## Test 2: Basic currency state transitions

Use an identified, uncorrupted, uninfluenced white body armour. Copy and import
it after each cheap in-game action:

1. Before crafting: expect `Normal` with zero explicit mods.
2. Apply an Orb of Transmutation: expect `Magic` with one or two explicit mods.
3. If it has one mod, apply an Orb of Augmentation: expect `Magic` with two
   explicit mods.
4. Apply an Orb of Scouring: expect `Normal` with zero explicit mods.

At every step, the base ID and item level must remain unchanged. Any quality,
sockets, generic implicits, and enchantments must also remain unchanged.

## Test 3: Crafted-mod round trip

Use a non-full Rare body armour:

1. Copy and import it before bench crafting; note the explicit-mod count and
   `crafted: false`.
2. Add one legal prefix or suffix at the crafting bench.
3. Copy and import it again; expect the count to increase by one and
   `crafted: true`. The crafted mod should be listed separately as `Crafted`.
4. Remove the crafted modifier at the bench and import once more; expect the
   original count and `crafted: false`.

This confirms both the game's advanced crafted header and explicit affix
capacity are represented correctly.

## Test 4: Essence reroll invariants

Use a disposable, unfractured body armour with quality and recognizable socket
colors. Record its explicit mods, then apply a Screaming Essence of Greed and
copy it again.

Expected:

- the item is Rare with four to six explicit mods;
- the essence life prefix is present;
- the previous ordinary explicit mods are gone;
- base ID, item level, quality, sockets, generic implicits, and enchantments are
  unchanged.

This validates the model's reroll boundary. The optimizer samples possible
reroll outcomes, so its exact randomly generated companion mods are not
expected to match the one live roll.

## Test 5: Fracture and corruption safety

For a cheap item with exactly one fractured explicit mod, record the state,
apply an Orb of Scouring, then copy and import it.

Expected:

- the fractured mod and its roll survive;
- all ordinary explicit mods are removed;
- the item is Magic because the fracture remains.

Separately, copy a disposable corrupted body armour that does not already have
the validation goal's exact crafted-life modifier, then run:

```powershell
Get-Clipboard -Raw | cargo run --quiet --release -- --goal goals\corrupted_item_validation.toml --item-file - --seed 42
```

Expected:

- the item reports `Corrupted`;
- `outcome`/summary is provably impossible with an `item_not_craftable`
  per-want reason;
- termination is `impossible` after zero completed generations;
- no crafting path is returned.

If a corrupted starting item already satisfies every required goal, an
unchanged zero-step result is valid instead. That is why this check uses a
known-absent exact modifier.

## Test 6: Value-scaled goal reporting

The import-validation goal contains two preferred `per_unit` wants. Import a
body armour with one or more modifiers that contribute maximum Life, using the
zero-expansion command from "Before testing" so reporting reflects that exact
item rather than a searched successor.

Expected in `Goal satisfaction`:

- the Life entry says `per_unit`;
- `attained` equals the sum of `base_maximum_life` rolls across every matching
  imported modifier;
- `contribution` is `0.1 × min(attained, 500)`;
- the values are reporting facts and do not make the goal required.

For the lower-is-better half, use an item with a reduced-attribute-requirements
modifier. The second entry should be satisfied at `attained <= -10`, and its
contribution should be `max(0, 0 - attained)`. This directly checks negative
stat handling and the lower-is-better cap.

## Test 7: Search limits and seed replay

With the Triumphant Lamellar capture or a live body-armour paste, run:

```powershell
cargo run --quiet --release -- --goal goals\import_validation_body_armour.toml --item-file triumphant_lamellar.txt --expansion-limit 0 --seed 42
```

Expected: `Search finished: expansion_limit after 0 generation(s)` and
`resolved seed 42`. Then compare an unrestricted run with a high-limit run,
both using seed 42; their reported path and final item should match:

```powershell
cargo run --quiet --release -- --goal goals\import_validation_body_armour.toml --item-file triumphant_lamellar.txt --seed 42
cargo run --quiet --release -- --goal goals\import_validation_body_armour.toml --item-file triumphant_lamellar.txt --expansion-limit 1000000 --seed 42
```

Hard-budget input and cooperative cancellation are service APIs rather than
CLI controls today. Their executable checks live in
`tests/application_service.rs`; repeat these manually once the desktop adapter
exposes them.

## Report a mismatch

Save the complete before/after clipboard text, the exact command, and the full
optimizer output. Do not trim advanced modifier headers or warning lines; they
are usually the fastest way to identify whether a mismatch is in importing,
eligibility, or crafting behavior.
