# `acct_10` — an account corpus with declared truth

20 records across three sources (`crm`, `vendors`, `billing`) resolving to
**10 accounts**, with ten designed traps. It exists because `acct_base` — the
only account corpus before this one — is seven records with no ground truth and
no traps, which is too thin to calibrate anything.

`truth.csv` gives each input row its account label; `traps.csv` indexes each
designed trap to the rows that construct it, so a fixture edit that dissolves a
trap fails a test instead of silently weakening the corpus. Both are read only
by tests — the pipeline never sees them, and `persona_id` is stripped at
delivery by `er_ingest::landing`.

There is no `scenario.yaml` and no `expected/`. The replay harness those belong
to was part of the removed Python implementation (`fixtures/static/FORMAT.md`
still describes it; nothing reads it). More importantly, committing an expected
membership would bake in a partition that the tests below show to be **wrong**
at the shipped score scale.

## The traps

Five push toward a missed duplicate, five toward a false merge. That balance is
what makes a cluster count informative: a corpus with only recall traps makes
every louder scale look better.

| Trap | Rows | What it tests |
|---|---|---|
| `legal_suffix_variants` | c1, v1, b1 | "Acme Inc." / "ACME, LLC" / "Acme Incorporated" — one company, three suffix conventions |
| `acquisition_rename` | c2, b2 | renamed after acquisition; domain, phone and postal stable, name unrecognisable |
| `dba_alias` | c4, b4 | trading name ("ITCH Systems") bearing no resemblance to the legal name |
| `typo_domain` | c4, b4 | `intiech.io` for `initech.io` — one transposition |
| `gray_band_pair` | c10, v10 | deliberately marginal: near-identical name, one side's phone missing |
| `missing_phone` | v10 | a blocking key absent on one side of a true pair |
| `shared_switchboard` | c8, v8, c9, b9 | **two distinct companies** sharing a phone number and a coworking address |
| `subsidiary_distinct` | c2, b2, c3, v3 | a subsidiary at the parent's address, with a name sharing its prefix |
| `same_name_distinct_company` | v6, b7 | two unrelated "Summit Partners" — a law firm and a landscaper |
| `placeholder_domain` | c5, c10, v10 | two unrelated companies both carrying `example.com` |

## What it measured

Driven by `er-cli/tests/acct_e2e.rs`, over a config compiled from
`config.yaml` (which is, unlike `acct_base` and `acct_test`, coherent with
`acct_model_v1`'s four comparisons and three blocking rules):

| `full_agreement_bits` | clusters (truth = 10) |
|---|---|
| 10, 15 | 20 — nothing merges |
| 21.8 (what the account model measures) | 13 |
| 30 – 43 | 11 |
| **44 – 90** | **9 — over-merges** |
| 120 | 8 |

At the shipped default of 60, Hooli Systems and Vandelay Industries become one
account. **No value reproduces 10**, because the true `dba_alias` duplicate and
the false `shared_switchboard` pair carry byte-identical evidence against these
four comparisons — the same gamma vector, the same per-field weights, the same
total of 4.3124318971685955 bits. Nothing monotone separates them.

Adding a near-match level on `domain_norm` (`config_domain_jw.yaml`) does
separate them — 12.0 bits against 4.3 — because a transposed domain is close and
an unrelated one is not. That alone still resolves to 9, since `auto_merge: 0.95`
admits both; the corpus resolves **exactly** at 44 bits + the domain level +
`auto_merge: 0.99`. All three have to move together.
