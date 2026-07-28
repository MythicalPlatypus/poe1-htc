# Understanding Results

This page walks through a real report line by line, then explains how to
act on the numbers. **Read this before spending expensive currency** — the
tool reports honest uncertainty, and knowing which number is which will
save you orbs.

The example below is `goals/finish_fractured_chest.toml --seed 42`: finish
a rare Astral Plate that already has a fractured T1 life roll, targeting
life + triple resistance.

## The header

```text
Loaded 39292 mods, 5059 base items from data
Data provenance: RePoE version unknown, fingerprint repoe-bundle-v1:sha256:<64 lowercase hex characters>
Crafting catalogs: 774 bench recipes, 106 essences, 445 fossils
Base item: Astral Plate (Metadata/Items/Armours/BodyArmours/BodyStr15), item level 86
Search: beam_width=25, max_steps=8, cost_weight=0.05, restart_cost=1c, seed=42
Budget: unbounded (restart_adjusted_expected cost shown for comparison)
Starting item: Rare with 2 existing mod(s) (1 fractured, crafted: false)
Search finished: target_reached after 2 generation(s), 8 ms; resolved seed 42
```

Sanity-check this section first: the right data fingerprint, base, item level,
starting mods, and effective search settings (CLI flags override the goal file,
which overrides defaults). RePoE version `unknown` means no trusted
`repoe-version.txt` sidecar was installed; the fingerprint still identifies
the exact JSON bundle used for the result. The final line says why the run
stopped and records the one seed that replays it. Other normal reasons are
`step_limit`, `expansion_limit`, `timed_out`, `cancelled`,
`search_exhausted`, and pre-search `impossible`.

## The recommended path

```text
=== Best crafting path: target COMPLETE (4/4 required, 4/4 total wants, goal score 34.0/34.0, ranking score -28.019) ===
  1. Chaos Orb — 1.00c per application; ~6.0% per try, reroll until hit -> ~16.7c expected
  2. Exalted Orb — 100.00c per application; one-shot, ~9.5% chance of >= this result
```

- **`target COMPLETE (4/4 required, 4/4 total wants)`** — the plan's final
  item satisfies every required want. Preferred wants (`required = false`) can
  remain unsatisfied without changing `COMPLETE`; the total count and score
  show how many it also landed. `INCOMPLETE` means at least one required want
  is still missing.
- **`goal score`** — sum of each want's contribution (here all 34 points).
  Presence/threshold goals contribute their weight when satisfied; `per_unit`
  goals contribute their value-scaled amount. `/unbounded` replaces the
  denominator when no sound maximum is known.
- **`ranking score`** — goal score minus `cost_weight ×` restart-adjusted
  expected cost, not the optimistic expected-cost line below. Complete targets
  always sort ahead of incomplete states; ranking score orders paths within
  the same completion class. It can be negative for expensive plans.

Each step is one of two kinds, and the difference matters:

- **Repeatable ("reroll until hit")** — you spam this until you hit the
  shown outcome or better. The Chaos Orb here hits ~6% per try, so it is
  priced at `1c / 0.06 ≈ 16.7c` expected. No risk of ruining the item —
  only of spending more than the average.
- **One-shot** — you pay once and live with the result. The Exalted Orb
  slam lands a result at least this good ~9.5% of the time. A miss does
  not refund your orb, and may leave the item worse than planned.

## The cost bracket

```text
Cost if every step hits first try: 101.0 chaos
Expected cost (rerolling repeatable steps until they hit): ~116.7 chaos
Chance all one-shot steps land at least this well: 9.5%
Expected cost if a one-shot miss scraps the item and you restart: ~1240.4 chaos (includes configured reset cost)
```

Read these as modeled scenarios from optimistic to pessimistic, not guaranteed
bounds:

1. **First-try cost** — lucky floor. You will rarely pay this little.
2. **Expected cost** — the realistic number *if all one-shots land*:
   repeatable steps priced at cost ÷ hit-chance.
3. **One-shot odds** — the chance the whole plan lands as printed. 9.5%
   means you should expect to miss more often than not.
4. **Restart estimate** — a pessimistic scenario: every one-shot miss scraps
   the item and you start over, paying `restart_cost` each time.

Your real cost may land between #2 and #4 because actual recovery after a
missed slam (annul, live with it, adjust the plan) is often cheaper than a full
restart but not free. It can also fall outside either estimate. The tool does
not yet model recovery policies — this is listed in Known Limitations.

Application-service callers can bind a chaos-equivalent hard cap to first-try,
retry-expected, or restart-adjusted expected cost. A compliant completion is
`COMPLETE`; a goal-complete exemplar beyond the selected cap is
`COMPLETE, OVER BUDGET` and prints its excess. This constrains a modeled
expectation, not guaranteed wallet consumption. Saved JSON reports all three
cost values and each one's `under`, `over`, or `not_comparable` comparison.

```text
(Sampled step probabilities are estimates; full rerolls use 50 Monte Carlo samples. ~ on costs and 1-in-N odds denotes approximation.)
```

A `~` before a percentage on a step marks a sampled probability. Full rerolls
(Chaos Orb, Alchemy, essences, fossils) use 50 samples per expansion, so a
`~6.0%` has real sampling noise and outcomes rarer than 1-in-50 can be
invisible to a single run. Methods such as Exalts, Augments, Regals, bench
crafts, Conqueror/Eldritch Exalts, and Bestiary swaps enumerate modifier
identities but sample numeric rolls, so roll-sensitive hit chances are also
estimates. Roll-insensitive removals such as Annulment and Fracturing are clean
exact examples.

The symbol also appears on expected-cost lines because those are expectations,
and tiny odds render as `~1 in N` for readability even when the underlying
identity probability was enumerated exactly. Eldritch Chaos has an additional
model assumption: it samples uniformly across legal replacement-affix counts
because RePoE does not publish the real count distribution.

## The final item and goal report

```text
--- Final item (Rare) ---
Prefixes:
  Carapaced [LocalIncreasedPhysicalDamageReductionRating6]  local_base_physical_damage_reduction_rating = 104
  ...
Fractured:
  Prime [IncreasedLife12]  base_maximum_life = 180

--- Goal satisfaction ---
  [x] [required, threshold, attained=180, contribution=10.000] stat base_maximum_life >= 175 (weight 10)
  [x] [required, threshold, attained=42, contribution=8.000] stat base_fire_damage_resistance_% >= 30 (weight 8)
  ...
```

This is the *planned* final item — the outcome the probabilities refer to,
shown with each mod's RePoE ID and rolled values. The bracketed IDs are the
same identifiers you use in `[[wants]]` and `[[item.mods]]`, so reports are
also the easiest way to learn the mod vocabulary. Imported items additionally
list quality, sockets, implicits, and enchantments here. Each goal line also
shows required/preferred status, scoring mode, aggregated numeric value, and
the exact score contribution.

## Alternative pathways

```text
--- Alternative pathway #2 (complete, goal 34.0/34.0, 4/4 required, 4/4 total wants, ranking -69.399, retry ~156.7c, restart ~2068.0c, one-shot odds 5.7%) ---
  Chaos Orb, then Orb of Annulment, then Exalted Orb

--- Alternative pathway #3 (incomplete, goal 26.0/34.0, 3/4 required, 3/4 total wants, ranking 25.167, retry ~16.7c, restart ~16.7c, rerolls only) ---
  Chaos Orb
```

With `--top N` you get up to N genuinely distinct routes, best first. They
are often more useful than the single best line: pathway #3 here scores
lower on the raw goal but has a numerically higher ranking score. It still
sorts after the complete routes because completion is the first priority.
Pathway #3 is **rerolls only** — no one-shot risk at all, ~17c, and gets 3 of
4 wants. Depending on your budget and stomach, that may be the plan you
actually execute.

## Is the recommendation stable?

Beam search is a heuristic and sampled probabilities carry noise. Before
committing real currency to an expensive plan:

- **Re-run with different seeds** (`--seed 1`, `--seed 2`, …). A robust
  recommendation survives seed changes; a coin-flip plan will flicker.
- **Widen the beam** (`--beam-width 100`). If the answer improves, the
  default width was too narrow for this goal; keep widening until it
  stabilizes.
- **Compare `--top 3`** routes. If they disagree wildly in strategy but
  score similarly, the model is telling you several plans are close — pick
  the one whose risk profile you prefer.

One more honest caveat: multi-step probabilities group sibling outcomes first
by required completion and then by goal score. Two equally ranked outcomes can
support different follow-ups, so a multi-step "chance all steps land" is a
policy estimate, not a guarantee that every counted branch continues exactly
as printed.

## Quick reference: tuning knobs

| Symptom | Try |
|---|---|
| Plan stops short of the full goal | Raise `max_steps`, widen `beam_width`, check the wants are individually reachable on this base/ilvl |
| Recommends absurdly expensive routes | Raise `cost_weight`, set real `[prices]` |
| Recommends cheap routes that barely improve the item | Lower `cost_weight` |
| Different runs give different answers | Set `--seed`, widen `beam_width`, re-check with several seeds |
| Search is slow | Lower `beam_width`/`max_steps`; make sure you built with `--release` |
