# Getting Started

This guide takes you from nothing to your first crafting plan. Expect it to
take about ten minutes, most of which is the first compile.

PoE1 HTC is a command-line tool: you describe the item you want in a small
text file, and it searches for the cheapest crafting route to get there. No
account access, no game hooks — it works entirely from
[RePoE](https://repoe-fork.github.io/) game-data exports and text you paste.

## 1. Install the prerequisites

You need **Git** and a **Rust toolchain**. Both are one-time installs.

- Git: <https://git-scm.com/downloads>
- Rust: <https://rustup.rs> — accept the defaults. On Windows, rustup may ask
  to install the Visual Studio C++ Build Tools first; let it.

Verify both from a fresh terminal:

```bash
git --version
cargo --version
```

## 2. Get the code

```bash
git clone https://github.com/MythicalPlatypus/poe1-htc.git
cd poe1-htc
```

## 3. Download the game data

The RePoE JSON exports are large and change every patch, so they are not in
the repository. Download them into the `data/` directory.

**Windows (PowerShell):**

```powershell
New-Item -ItemType Directory -Force data | Out-Null
curl.exe -o data/mods.json https://repoe-fork.github.io/mods.json
curl.exe -o data/base_items.json https://repoe-fork.github.io/base_items.json
curl.exe -o data/crafting_bench_options.json https://repoe-fork.github.io/crafting_bench_options.json
curl.exe -o data/essences.json https://repoe-fork.github.io/essences.json
curl.exe -o data/fossils.json https://repoe-fork.github.io/fossils.json
```

**Linux / macOS / Git Bash:**

```bash
mkdir -p data
curl -o data/mods.json https://repoe-fork.github.io/mods.json
curl -o data/base_items.json https://repoe-fork.github.io/base_items.json
curl -o data/crafting_bench_options.json https://repoe-fork.github.io/crafting_bench_options.json
curl -o data/essences.json https://repoe-fork.github.io/essences.json
curl -o data/fossils.json https://repoe-fork.github.io/fossils.json
```

Only `mods.json` and `base_items.json` are strictly required. The other three
let the tool resolve names like `Pristine Fossil` and `Deafening Essence of
Greed` and validate bench crafts — you want them.

When a new league launches, re-run these downloads to refresh the data.

## 4. Build and verify

```bash
cargo build --release
cargo run --release
```

The first build takes a few minutes. Running without arguments performs a
data check; you should see something like:

```text
POE1 HTC — Crafting Path Optimizer
Loaded 39292 mods, 5059 base items from data
Crafting catalogs: 774 bench recipes, 106 essences, 445 fossils

No --goal file given; data check complete.
```

Always use `--release`. Debug builds work but the search is many times
slower.

## 5. Run your first search

The repository ships example goals. Try the life + fire-res chest:

```bash
cargo run --release -- --goal goals/example_life_chest.toml --seed 42
```

You will get a recommended sequence of crafting steps, their estimated cost
in chaos, and the chance that risky steps land. `--seed 42` makes the run
reproducible — same seed, same answer — which is useful when comparing
settings.

Two more examples worth reading:

- [`goals/finish_fractured_chest.toml`](../goals/finish_fractured_chest.toml)
  — finish a craft you already started, protecting a fractured mod.
- [`goals/mirror_tier_chest.toml`](../goals/mirror_tier_chest.toml) — a
  deliberately hard six-mod target that shows how one-shot risk is reported.

## 6. Point it at your own item

If the item you want to finish is sitting in your stash, you do not have to
describe it by hand. Hover it in game, press `Ctrl+C`, paste into a text
file, and run:

```bash
cargo run --release -- --goal goals/example_life_chest.toml --item-file my_item.txt
```

See [Importing Your Item](importing-your-item.md) for the details.

## Where to go next

- [Writing Goal Files](writing-goals.md) — describe the item you want.
- [Importing Your Item](importing-your-item.md) — start from an item you own.
- [Understanding Results](understanding-results.md) — what the numbers mean
  **before you spend currency**.
- [FAQ & Troubleshooting](faq.md) — common errors and questions.
