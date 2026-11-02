# Profile findings

> **Generated** from the registry in `rust/crates/er-profile/src/rules/`.
> Do not hand-edit — `er-profile`'s own test byte-compares this file.

A profile describes a corpus; a finding says what about it is worth acting on. Nothing here refuses or gates a dataset — whether a 70%-empty column is a defect or simply how that source works is the data owner's call, so every rule reports and none blocks.

| Code | Severity | Invariant |
|---|---|---|
| P001 | warning | a profiled column holds at least one value |
| P002 | warning | a column used for matching is populated in at least half its rows |
| P003 | warning | no single value covers a large share of a column |
| P004 | error | a column holds no placeholder values standing in for absence |
| P005 | warning | a column's values do not collapse substantially under normalization |
| P006 | info | a text column holding a single consistent type is compared as that type |
| P007 | warning | a column holds more than one distinct value |
| P008 | error | a near-unique column is actually unique |
| P009 | warning | at least one column is a usable blocking key on its own |
| P010 | info | the most selective usable blocking key is named with its cost |

## P001 — a profiled column holds at least one value

*warning*

An entirely empty column is reported separately from a mostly-empty one because the two call for different actions and because the arithmetic of the second hides the first: a fill rate of 0% and a fill rate of 3% both read as "low", but only one of them means the field is never populated and is safe to drop from the mapping entirely. It is also the single most common consequence of a mis-mapped column — a source column spelled slightly wrong produces a canonical attribute that is empty in every row, and nothing else in the pipeline will say so. Counting blank apart from NULL is what makes this detectable at all: ingest lands every delivered value as a JSON string, so an empty cell arrives as '' and a NULL-only check would call the column fully populated.

**Quiet on:** `email`: 1000 rows read, 940 populated — 94% full.

**Fires on:** `middle_name`: 1000 rows read, 0 populated. Every row is NULL or blank, so the column is carrying no information and any comparison on it is pure cost.

## P002 — a column used for matching is populated in at least half its rows

*warning*

Sparsity costs recall silently and quadratically. An equality is never true of a NULL, so a blocking key on a column with fill rate `f` reaches only `f^2` of the corpus's pairs — at 41% full that is 17%, and the other 83% of pairs are not scored, not reviewed, and not reported anywhere. The square is the part people get wrong: a column that is "only" 40% empty feels tolerable and actually discards nearly two thirds of the pairs. The same sparsity also degrades training, because a comparison level that almost never fires accumulates too few observations to estimate from, and an unobserved level is read as a tiny probability rather than as an absence of evidence. This rule reports the fill rate and the implied pair loss together, because the second is the number that makes the first actionable.

**Quiet on:** `phone_e164`: 1000 rows, 820 populated — 82% full, so a key on it still reaches about 67% of all pairs.

**Fires on:** `addr_postal`: 1000 rows, 410 populated — 41% full. A blocking key on this column can never reach 83% of the corpus's pairs, because both endpoints of a pair have to carry the value.

## P003 — no single value covers a large share of a column

*warning*

Blocking cost is not driven by the average block, it is driven by the largest one. Pairs inside a block grow with its square, so a value covering share `s` of `n` rows contributes on the order of `(s*n)^2 / 2` comparisons by itself — which means one pathological value can dominate a run while the column's total pair count still looks affordable. The classic shape is a placeholder: every record with a blank-but-present postcode lands in one enormous block. The same skew also defeats match weighting unless term-frequency adjustment is switched on for the column, because agreeing on a value half the corpus shares is weak evidence being scored as though it were strong. Reported with the value and its share so the fix — exclude it, normalize it, or enable TF — can be chosen rather than guessed.

**Quiet on:** `surname`: the most frequent value is `Jones` at 23 of 819 rows (2.8%) — a long tail, no single block worth worrying about.

**Fires on:** `addr_city`: the most frequent value covers 31% of rows. Blocking on this column would generate roughly 5% of the entire cartesian product from that one value.

## P004 — a column holds no placeholder values standing in for absence

*error*

A placeholder is worse than a NULL, because it is a value: `unknown` equals `unknown`, so every record carrying it agrees with every other one. That turns absence into positive evidence of a match — the records block together, score as agreeing on the field, and the resulting cluster is wrong in a way no threshold can fix, because the model was told the truth about what it was given. A NULL, by contrast, is handled correctly everywhere: it fails the equality, lands on the null comparison level, and contributes nothing. So the remedy is almost always to map the placeholder to NULL at standardization rather than to tune anything. The threshold is deliberately low because this is a correctness problem at any volume, and the placeholder list is deliberately short because a longer one starts matching real surnames and place names. Graded `error` rather than `warning` because the consequence is a WRONG RESULT and not a cost: records sharing nothing but their missing data get merged, and an over-merge is the expensive mistake in entity resolution — far harder to notice, and far harder to undo, than a duplicate left behind.

**Quiet on:** `addr_region`: top values are `CA`, `TX`, `NY` — all real.

**Fires on:** `phone`: 4% of rows hold the literal `unknown`. Those records all compare equal to each other on phone and will be pulled into the same block and scored as agreeing.

## P005 — a column's values do not collapse substantially under normalization

*warning*

This is the one finding a raw distinct count actively hides. A column holding `New York`, `new york` and `New  York` looks healthy — three distinct values, high cardinality, a promising blocking key — and is in fact one city spelled three ways. The consequence is a recall loss that nothing else reports: an exact-match blocking key separates the variants into different blocks, so those records are never compared, never scored, and never appear in a review queue. Comparing the distinct count before and after lowercasing, trimming and collapsing internal whitespace is what makes it visible, and it is cheap because both counts come from the same pass. The remedy is a standardization transform, not a looser comparator: normalizing once at ingest fixes blocking too, whereas a fuzzy comparison level only helps pairs that blocking already produced.

**Quiet on:** `email`: 428 distinct raw, 428 distinct normalized — already clean.

**Fires on:** `addr_city`: 216 distinct raw, 180 distinct normalized. 36 of the distinct values (17%) differ from another only by case or spacing, so `New York`, `new york` and `New  York` are being treated as three different cities.

## P006 — a text column holding a single consistent type is compared as that type

*info*

Ingest stores every delivered value as text, so the declared type says nothing at all about content and type conformance is the only evidence of what a column actually holds. A column where every value parses as a date is a date being compared with string equality, and string equality on dates is wrong in a specific and silent way: `1990-01-02` and `1990-1-2` are the same day and different strings, while `02/01/1990` and `01/02/1990` are different days and equally plausible spellings. The same applies to numbers, where leading zeros and thousands separators decide equality. This is reported as information rather than a warning because text may be exactly right — an identifier that happens to be all digits must stay text or it loses its leading zeros — so the rule names what it found and leaves the choice of a date-aware comparator or a cast to the person who knows what the field means.

**Quiet on:** `given_name`: 0% parses as a date, 0% as a number — text holding text.

**Fires on:** `dob`: 1000 of 1000 values parse as a date, but the column is text and is compared with string equality, so `1990-01-02` and `1990-1-2` are different people.

## P007 — a column holds more than one distinct value

*warning*

A constant column is the degenerate case of skew and deserves naming separately, because its two failure modes are both total rather than gradual. As a blocking key it is equivalent to no blocking at all: every record agrees, so the key produces the entire cartesian product, and on a corpus of any size that is the difference between a run and a runaway. As a comparison it contributes exactly the same weight to every pair, which shifts every score by a constant and changes no ranking — pure cost for zero discrimination. Neither shows up as an error anywhere: the run just gets slower or the model just gets less useful. Worth checking against the sampled flag too, since a narrow sample of a genuinely varied column can look constant.

**Quiet on:** `source_system`: 4 distinct values over 1000 rows — low cardinality, but it does discriminate.

**Fires on:** `country`: 1 distinct value over 1000 rows. Every record agrees, so a blocking key on it is the full cartesian product and a comparison on it contributes the same weight to every pair.

## P008 — a near-unique column is actually unique

*error*

A column that is almost but not quite unique is the signature of a broken record identifier, and it is dangerous precisely because it looks fine: 997 distinct values in 1000 rows reads as a healthy identifier, and the three collisions silently decide which version of a record survives ingest. Graded `error` for that reason: the outcome is not a slower run or a missed duplicate but a row that is simply gone, overwritten by whichever copy landed last. Reported only when the EXACT distinct count was computed, never from the approximate one — `approx_count_distinct` is a HyperLogLog and is wrong in both directions (measured at 978 on a column of 1000 genuinely distinct ids), so inferring uniqueness from it would both invent collisions that do not exist and miss the handful that do. That is why `ColumnShape::is_unique` returns an Option and this rule stays silent rather than guessing: a false alarm here sends someone hunting duplicates in a clean column, and a missed one leaves records overwriting each other. The near-unique threshold is itself a stand-in for config: which column is MEANT to identify a record is declared, and once this pass can read that declaration the right check is an exact one on that column, where any collision at all is a finding and no threshold is needed.

**Quiet on:** `surname`: 900 distinct values over 940 rows (96%) — varied, but nobody expected a name to identify a record, so no collision is implied.

**Fires on:** `crm_id`: 997 distinct values over 1000 populated rows (99.7%). Three ids repeat, so the column cannot identify a record and whichever row lands last wins.

## P009 — at least one column is a usable blocking key on its own

*warning*

This is the one question a profile exists to answer before a model is trained, and it cannot be answered one column at a time: whether the corpus is blockable depends on the BEST column, so a per-column rule would either say nothing or say it twelve times. When no single column clears both bars the remedy is a composite key — `surname` plus `addr_postal` where neither alone is selective enough — and knowing that before the first run is the difference between a configuration decision and a three-hour surprise. Two bars rather than one because the failure modes are opposite: a key that eliminates too few comparisons makes the run quadratic, and a key that is cheap because its column is mostly empty makes the run miss most of its pairs. Silent when the candidates were not measured, because an empty candidate list means the tier did not run, not that nothing qualified.

**Quiet on:** `email` eliminates 99.6% of comparisons and loses 8% of pairs to NULLs — a usable key on its own.

**Fires on:** Across 12 measured columns the best key still leaves 14% of all possible comparisons, and the only selective column is 71% empty. No single column will block this corpus; it needs a composite key.

## P010 — the most selective usable blocking key is named with its cost

*info*

A findings panel that only ever reports problems is a panel people learn to dread and then to skip. This one carries the answer: of the columns measured, here is the key to block on and here is what it will cost. It is `info` rather than a warning because nothing is wrong — it exists so the most actionable line in the profile is present whether or not anything else fired. 'Most selective' is the lowest pair count among the keys that clear BOTH bars, never the lowest overall: the cheapest key on a corpus is very often a column that is almost entirely empty, and recommending that would be recommending a key that cannot see the data.

**Quiet on:** (Nothing to report: this rule only speaks when a viable key exists, and what it says is which one.)

**Fires on:** `email` is the most selective usable key: 2,140,882 pairs, 99.6% of comparisons eliminated, 8% of pairs unreachable because the column is NULL.
