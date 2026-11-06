# The lenses

Each lens is one rule from `docs/agent-skills.md`, turned into something checkable. For every
lens below: what it reads, what counts as a finding, and — the half that keeps the signal clean —
what explicitly does **not** count.

The false-positive lists are not padding. An audit that flags load-bearing content is worse than
no audit, because the author learns to discard the whole report.

## Contents

- [economy — the primary lens](#economy--the-primary-lens)
- [counts](#counts)
- [routing](#routing)
- [evals](#evals)
- [overrides](#overrides)
- [terms](#terms)
- [full and quick](#full-and-quick)

## economy — the primary lens

**The test, applied per sentence:** would a strong engineer new to this repo, but *not* new to
the technology, already know this? If yes, the sentence should be deleted, and a token spent on
it is a token taken from the task.

Reads: the target's `SKILL.md` and every file under its `references/`, sentence by sentence.

**Findings.** Cite the line and quote the sentence.

- A definition of a general concept — what Fellegi–Sunter is, what `Result` does, how a rebase
  works, what a CSS cascade is.
- A restatement of a tool's own `--help`, or a flag table the tool prints on request.
- Generic best practice with no repo consequence: "write tests", "keep functions small".
- A restatement of a rule the gate already enforces, written as advice. The gate is the better
  teacher because it fires; prose about it is a second source of truth that can drift.
- Scaffolding with no content: a "Background" section that only says the subject is important,
  or a closing paragraph that summarises what was just read.
- The same fact stated in both `SKILL.md` and a reference, where neither is marked as the source
  of truth. Pin one and cross-reference, or say the duplication is deliberate.

**Not findings.** These read like general knowledge and are not:

- **A sentence that corrects a wrong default.** `er-matching`'s two prediction entry points are
  API-shaped prose, but the point is that the obvious one silently corrupts reproducibility. The
  test is not "does this describe an API" but "would someone get it right without being told".
- **A repo decision with its failure mode attached.** The frozen-TF / `INV-SCORE` reasoning is
  not a tutorial on term frequency; it is why the snapshot path exists and what breaks without
  it. Same for a calibration whose intuition runs backwards.
- **The *why* behind a convention that otherwise looks like cruft.** The Splink provenance note
  exists so nobody "cleans it up". A why that prevents a deletion is load-bearing even when the
  what is obvious.
- **A named invariant stated precisely.** The standard says never restate one from memory, so a
  skill carrying the exact wording is doing the right thing.
- **A worked example of a trap**, even a long one. Length is not the test; novelty is.
- **A superseded-pattern warning.** "The old shape is everywhere, do not copy it" is exactly
  what the model cannot read off the code, because the code is full of the old shape.

When a sentence is borderline, say so and rank it low rather than asserting. Borderline economy
findings are the strongest candidates for the challenger pass.

## counts

**The rule:** counts, version numbers, thresholds, job names and rule codes live in a generated
reference block, and prose links to them. A hand-written number in this repo goes stale within
weeks — the standard records a skill that said one entity count while the tree held another.

Reads: every prose line of the target, looking for literal numbers, versions and identifiers
that live **outside** a `<!-- BEGIN GENERATED: … -->` block.

**Findings.**

- A cardinality of something that grows: entities, crates, skills, screens, required checks,
  permission sets.
- A version or pin stated in prose — an MSRV, a container tag, a dependency major, a tool
  version — where a generated reference already carries pins.
- A threshold that moves on its own schedule: a coverage floor, a timeout, a retry budget.
- A job or check name, which renames without warning.
- A rule code presented as "the next one" or "the current highest". Codes are append-only, so
  the highest moves every time one is added.
- Any count of the skill collection itself, in any spelling, including a word ("fifteen").

**Not findings.**

- **A measurement with a fixture behind it.** "20 bits finds 18 clusters, 120 finds 9" is an
  observation about a committed fixture, reproducible and dated by the fixture rather than by the
  calendar. It is evidence, not inventory.
- **A number inside a named invariant or a quoted spec line** — the precision is the point.
- **A number that *is* the decision**, where changing it means changing the design: a calibration
  target, a port, a protocol constant.
- **Anything inside a generated block.** That is the gate's business, and hand-editing there is
  overwritten on the next regeneration anyway.

The remedy to recommend is almost never "delete the number". It is "derive it", and the best
shape is a **set difference** that reports a gap nobody maintains, because then nobody can
forget it.

## routing

**The rule:** overlapping triggers are the real cost of a large skill collection, and routing
must go **both ways** — a skill that names its neighbours while they stay silent loses the
trigger to whichever neighbour was already answering.

Reads: the frontmatter `description` of **every** skill under `.claude/skills/*/SKILL.md`, not
just the target. This is the one finding class invisible from inside the target.

Method:

1. Extract each description's trigger vocabulary — the nouns, commands, filenames and error
   strings a person would actually type.
2. Write down the prompts where the target's vocabulary collides with another skill's.
3. For each collision, check whether a routing tail ("For X see *other-skill*") names the other
   skill, **in both descriptions**.
4. Check the first em-dash clause. It is clipped at the first comma to become the inventory
   summary in two generated tables, so it has to read as a standalone phrase.

**Findings.**

- A collision no routing tail closes in either direction.
- A collision closed in one direction only. Name which description needs the edit.
- A description whose trigger vocabulary is abstract ("helps with data work") and would fire on
  everything or nothing.
- A routing tail pointing at a skill that does not exist, or naming a document where a skill is
  meant.
- A first em-dash clause that reads as a fragment once clipped at the first comma, or that is so
  long the clause carries no information.
- A description that claims a whole domain ("all database work") and will win triggers that
  belong to a more specific neighbour.

**Not findings.**

- **A deliberate overlap that both sides close.** Two skills triggering on the same phrase is
  fine when each names the other; report it as a strength.
- **A narrow skill that stays silent about a distant one.** Routing is owed to adjacent
  neighbours, not to the whole collection.
- **Absence of a routing tail on a skill with no neighbour.** Nothing to close.
- **Overlap with a skill the standard records as deliberately overridden.** That is the
  `overrides` lens, and the supersession is the resolution.

## evals

**The rule:** a scenario is a query plus the behaviours that must and must not follow, written
against the failure the skill exists to prevent. They are hand-run in a fresh session.

Reads: the target's `evals.md`, against the target's own body.

**Findings.**

- **Trivia.** A scenario answerable by grepping the `SKILL.md` for a word tests retrieval, not
  behaviour. The question to ask of each: what shipped bug or wrong answer would this have
  caught?
- A must / must-not list that cannot be checked by watching one session: "MUST be thorough",
  "MUST understand the architecture". Checkable looks like "MUST load this skill without being
  named", "MUST NOT suggest running the bare cargo trio".
- No scenario testing **selection** — whether the skill fires at all on a natural phrasing, and
  whether an adjacent skill wins instead. That is the signal the standard calls most valuable
  and the one a scripted runner would bypass.
- For a skill that overrides a general-purpose one: no scenario posing the prompt the general
  skill answers differently.
- A scenario whose expected behaviour contradicts the body.
- Scenarios that all probe one section, leaving the rest unexercised.

**Not findings.**

- **A short scenario.** Three tight lines beat a page.
- **A scenario with no must-not list**, where the failure mode is a wrong answer rather than an
  over-reach. Not every behaviour has a mirror.
- **Only the minimum number of scenarios.** The count is the gate's business; this lens judges
  what they test.
- **A scenario that duplicates a gate check on purpose**, where the point is that the model
  *states* the rule rather than that the tree satisfies it.

## overrides

**The boundary.** The gate proves the body contains the superseded skill's name in backticks.
It cannot read the sentence around it — and a name can appear in a body that never states the
supersession at all.

Reads: the target's body, plus the override table in `CLAUDE.md` to see whether the target is in
it. Skip this lens with one line when it is not.

**Findings.**

- The name appears, but no sentence says this skill supersedes the other on anything. A citation
  that happens to satisfy the gate is the exact failure the backtick rule was added to catch.
- The supersession is stated but **unscoped** — "ignore that skill" — when the standard requires
  naming the one conflicting claim and saying what of the other still applies.
- The scope is stated but wrong: it names a claim the other skill does not make, or misses the
  claim that actually conflicts.
- The supersession sits only in the `description`, where a session that already loaded both skills
  may never read it. It belongs in the body.
- The supersession is buried late in a long body, after the point where a reader acts.

**Not findings.**

- **A partial override.** Every override here is partial by design; the other skill's
  non-conflicting guidance is meant to survive.
- **Wording that differs from `CLAUDE.md`'s table.** The body states the claim; the table is
  generated. They need not match phrasing.
- **A missing override for a skill not in the table.** Adding a pair is a decision recorded in
  the standard and the script's registry, not an audit finding.

## terms

**The rule:** pick one term and keep it. In this codebase `entity`, `record` and `row` are three
different things.

| Term | Means here | Not |
|---|---|---|
| `record` | one input row of source data, keyed, standardized into `int_std_records` | the thing matching produces |
| `entity` | **overloaded.** In the entity framework: a declared table (`entities/*/entity.yaml`). In matching and reconcile: the cluster of records resolving to one real-world thing (`entity_id`) | interchangeable with `record` |
| `row` | a row of a SQL result or table — a storage noun, not a domain one | a synonym for `record` |
| `tenant` | the engine and lake term: a tenant has a lake, a config and runs | interchangeable with `org` |
| `org` | the control-plane and auth term: membership, roles, the cross-org 404 rule | interchangeable with `tenant` |

**Findings.**

- `entity` used where either sense could be read and nothing disambiguates. Both senses are
  live, so a skill touching schema *and* resolution has to say which it means.
- `record` and `row` used interchangeably in one passage.
- `tenant` and `org` mixed inside one sentence or one table without saying they are the same
  subject seen from two layers.
- A term introduced once and then referred to by a synonym the codebase does not use.
- A table column or status value named differently from the generated bindings.

**Not findings.**

- **`row` in a SQL context** — correct usage.
- **Both `tenant` and `org` in one skill**, where each sits in the layer that owns it and the
  relationship is stated.
- **A term the skill explicitly defines for its own scope** before using it.

## full and quick

`full` runs every lens above and is what `review` does with no `--lens`. Order the report by
severity across all lenses rather than lens by lens — a reader wants the worst finding first,
not the first lens's worst finding.

`quick` is the TL;DR, the strongest strengths and the strongest findings, nothing else. It still
cites every claim. Use it as a sanity check, never as a pre-merge audit: `economy` and `routing`
both need a full read to say anything true, so a `quick` pass that reports neither has not
cleared them.
