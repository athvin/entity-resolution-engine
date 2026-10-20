# DataGroomr — feature teardown

*Research date: 2026-10. Every capability DataGroomr ships, rewritten without Salesforce and stack-ranked by how much it matters to a client who wants one thing: their records deduplicated into one entity per person. Features and pricing drift over time; see [Sources](#sources). Companion to [research-cloudingo.md](research-cloudingo.md).*

## Evidence base

| Source | Count |
|---|---|
| Release-note posts | 50 |
| Monthly changelogs | 76 (1,938 items) |
| Help articles | ~165 |
| Blog posts read | 102 of 200 |
| Product and pricing pages | ~30 |
| Independent reviews | 19 |
| Capabilities ranked | 88 |

## What the research says

- **35 of 88** capabilities carry the dedupe value (tiers 1-2): matching, clustering, blocking, survivorship, review, bulk resolution, reversibility, cross-source matching and the accuracy controls around them.
- **29 + 24.** 29 generic capabilities lean on Salesforce in DataGroomr's implementation, and 24 more features are pure Salesforce artifacts. Undo depends on the Salesforce Recycle Bin, real-time matching on Salesforce CDC, and every merge deletes the losing record.
- **3 h** hard limit per analysis run, with the documented fix being to split datasets. Blocking is not exposed and API throughput is 60 requests a minute. Scale and cost are where DataGroomr strains.
- **20+** enrichment providers added since May 2026, plus an AI agent. The 2026 roadmap went to upsell features, while explainability arrived only in May 2026 and multi-model matching is capped at three.
- **#1** customer complaint is the learning curve for matching and merge rules. Ease of setup is also DataGroomr's top-rated trait, so whoever makes survivorship configuration simplest wins the comparison.

## The stack rank

Seven tiers, ranked 1 to 88. Tier 1 is what a dedupe buyer evaluates first; tier 7 has little to do with dedupe quality.

| Tier | Name | Ranks | Why it is where it is |
|---|---|---|---|
| T1 | [Core matching & resolution](#tier-1-core-matching--resolution) | #1–#16 | Without these the product does not dedupe. Every buyer evaluates these first. |
| T2 | [Accuracy, tuning & trust](#tier-2-accuracy-tuning--trust) | #17–#35 | Separates a tool that finds duplicates from one a client will let auto-merge. |
| T3 | [Prevention & integration](#tier-3-prevention--integration) | #36–#48 | Stops duplicates from coming back and lets the engine sit inside the client's systems. |
| T4 | [Operating at scale](#tier-4-operating-at-scale) | #49–#59 | Automation, audit and safety rails for running dedupe as a standing process. |
| T5 | [Input hygiene (cleanse & verify)](#tier-5-input-hygiene-cleanse--verify) | #60–#73 | Adjacent features whose main value to a dedupe buyer is better match keys and survivorship tie-breakers. |
| T6 | [Governance & platform](#tier-6-governance--platform) | #74–#80 | Procurement gates. Low daily value, but they block enterprise deals if missing. |
| T7 | [Peripheral & upsell](#tier-7-peripheral--upsell) | #81–#88 | Revenue add-ons and polish. Little to do with dedupe quality. |

### Match-confidence bands

The matrix refers to three bands:

- **Auto-accept band** — high confidence; resolve without review.
- **Review band** — gray zone; a human or LLM adjudicates.
- **Reject band** — below threshold; not the same entity.

## Feature matrix

Each row is a platform-agnostic capability; *DataGroomr name* is what DataGroomr calls it. *SF dependency* shows how tied DataGroomr's implementation is to Salesforce (None / Partial / SF-only). Each capability's detail — what DataGroomr does, the generic equivalent and why it sits at that rank — follows in [Capability detail](#capability-detail).

| # | Capability | DataGroomr name | Tier | Module | SF dependency | First shipped | Plan gating |
|---|---|---|---|---|---|---|---|
| 1 | [Probabilistic / ML pairwise matching](#1-probabilistic--ml-pairwise-matching) | Machine Learning matching models | T1 | Dedupe | None | 2019 core; custom models Nov 2020 | All plans |
| 2 | [Clustering pairs into entities](#2-clustering-pairs-into-entities) | Matched duplicate groups | T1 | Dedupe | None | 2019 core; in-cluster scoring Jan 2026 | All plans |
| 3 | [Candidate generation (blocking) at scale](#3-candidate-generation-blocking-at-scale) | Implicit blocking; 'broader blocking' in Similar v2 | T1 | Dedupe | None | Core; widened Jan 2026 | All plans |
| 4 | [Deterministic & fuzzy rule matching](#4-deterministic--fuzzy-rule-matching) | Classic matching models (Exact / Similar) | T1 | Dedupe | None | Nov 2022; fuzzy Dec 2024 | All plans |
| 5 | [Calibrated confidence & threshold bands](#5-calibrated-confidence--threshold-bands) | Match Confidence; Minimum Match Confidence; dual slider | T1 | Dedupe | None | May 2020; dual slider May 2025 | All plans |
| 6 | [Survivor (master record) selection rules](#6-survivor-master-record-selection-rules) | Master Record Rules | T1 | Dedupe | None | Sep 2020 | All plans |
| 7 | [Attribute-level survivorship](#7-attribute-level-survivorship) | Field Merge Rules (Field Value Rules) | T1 | Dedupe | None | Oct 2020; distribute Apr 2021; advanced logic Nov 2025 | Advanced rules: Professional+ |
| 8 | [Human review queue (side-by-side)](#8-human-review-queue-side-by-side) | Review dialog | T1 | Dedupe | None | 2019 core | All plans |
| 9 | [Bulk resolution of clusters](#9-bulk-resolution-of-clusters) | Mass Merge | T1 | Dedupe | None | 2019 core; filters May 2025; parallel Nov 2026 | All plans (trial capped at 25 merged records) |
| 10 | [Re-point related records to the survivor](#10-re-point-related-records-to-the-survivor) | Merge with re-parenting; related-record handling | T1 | Dedupe | Partial | 2019 core; edge cleanup Nov 2020 | All plans |
| 11 | [Reject decisions remembered (must-not-link)](#11-reject-decisions-remembered-must-not-link) | Unmatch; Mass Unmatch; Undo Unmatch | T1 | Dedupe | None | 2019; mass Nov 2025 | Undo Unmatch: Enterprise |
| 12 | [Undo a merge](#12-undo-a-merge) | Undo Merge; Undo All | T1 | Audit | Partial | Sep 2021; Undo All Aug 2023 | Add-on; included in Enterprise |
| 13 | [Full rollback including field values](#13-full-rollback-including-field-values) | Rollback Merge | T1 | Audit | Partial | Dec 2023 / Jan 2024 | Separate add-on |
| 14 | [Cross-source matching](#14-cross-source-matching) | Cross-object datasets | T1 | Dedupe | Partial | Jun 2020; any object May 2022 | Professional+ |
| 15 | [Pre-match normalization](#15-pre-match-normalization) | Basic preprocessing; name, company, address parsing | T1 | Dedupe | None | Jul 2020; opt-out May 2026 | All plans |
| 16 | [Synonym, nickname & stop-word dictionaries](#16-synonym-nickname--stop-word-dictionaries) | Dictionaries (Synonym / Ignore Words) | T1 | Dedupe | None | Jan 2025 | Professional+ |
| 17 | [Per-pair match explanation](#17-per-pair-match-explanation) | Match Confidence transparency | T2 | Dedupe | None | May 2026 | All plans |
| 18 | [Hard match constraints](#18-hard-match-constraints) | Required match (Classic); Required Field (ML) | T2 | Dedupe | None | Jun 2025; ML Aug 2025 | All plans |
| 19 | [Active-learning labeling](#19-active-learning-labeling) | Train manually (Yes / No / Not Sure); Coverage metric | T2 | Dedupe | None | Nov 2020; coverage Aug 2024 | All plans |
| 20 | [Learning from reviewer decisions](#20-learning-from-reviewer-decisions) | Auto training | T2 | Dedupe | None | Nov 2020 | All plans |
| 21 | [Configurable field weights](#21-configurable-field-weights) | Classic field weights | T2 | Dedupe | None | Aug 2023 | All plans |
| 22 | [Null / blank handling policy](#22-null--blank-handling-policy) | Blank value options | T2 | Dedupe | None | Sep 2024 (blank matching); three options Dec 2024 | All plans |
| 23 | [Cross-field comparison](#23-cross-field-comparison) | Field groups; Cross Compare Fields | T2 | Dedupe | None | Aug 2024 | All plans |
| 24 | [Derived match keys (pre-match transforms)](#24-derived-match-keys-pre-match-transforms) | Transform rule in preprocessing; First N characters | T2 | Dedupe | None | First N Aug 2024; transforms May 2025 | Pre-match transforms on pricing grid (tier not readable) |
| 25 | [Type-aware similarity (dates, numbers, geo, address)](#25-type-aware-similarity-dates-numbers-geo-address) | Similar for dates, numbers, locations; whole address fields | T2 | Dedupe | None | Dec 2025; Nov 2026 | All plans |
| 26 | [Features from related records](#26-features-from-related-records) | Related-object fields in models (up to 2 hops) | T2 | Dedupe | Partial | Jan 2024; 2 hops Jul 2026 | All plans |
| 27 | [LLM adjudication of gray-zone groups](#27-llm-adjudication-of-gray-zone-groups) | AI Recommendations (merge, manual merge, unmatch, link, split) | T2 | AI | None | Sep 2025 | Credits (free with BYO LLM) |
| 28 | [Zero-config starter models](#28-zero-config-starter-models) | Pre-trained models; AI-built models at sign-up | T2 | Dedupe | None | Oct 2020; AI onboarding Mar 2025 | All plans |
| 29 | [Natural-language rule authoring](#29-natural-language-rule-authoring) | Design with AI; Explain Rule; AI-generated rules | T2 | AI | None | May 2025; v2 Nov 2025; Explain Dec 2025 | All plans |
| 30 | [Rule preview on real records](#30-rule-preview-on-real-records) | Rule Preview with what-if editing | T2 | Dedupe | None | Apr 2022; what-if Aug 2026 | All plans |
| 31 | [Ensembles & cascades of models](#31-ensembles--cascades-of-models) | Multi-model matching (up to 3) | T2 | Dedupe | None | Apr 2025 | Enterprise |
| 32 | [Relationship exclusions](#32-relationship-exclusions) | Exclude same-hierarchy accounts | T2 | Dedupe | Partial | Dec 2025 | All plans |
| 33 | [Household / group entities](#33-household--group-entities) | AI 'Link' recommendation; NPSP household handling | T2 | Dedupe | Partial | Sep 2025 (link); NPSP Apr 2025; households Aug 2026 | NPSP merge: Professional+ |
| 34 | [Model versioning & A/B](#34-model-versioning--ab) | Model versions; Similar v1 pin; clone across types | T2 | Dedupe | None | Oct 2025; pin Jan 2026 | All plans |
| 35 | [AI-assisted model setup & training](#35-ai-assisted-model-setup--training) | AI field selection; Train with AI | T2 | AI | None | Mar 2025; Apr 2025 | All plans |
| 36 | [Real-time incremental matching](#36-real-time-incremental-matching) | Live Dedupe | T3 | Dedupe | Partial | Aug 2025; GA Dec 2025 | Pro 1 dataset, Enterprise 3; $500/yr per extra |
| 37 | [Online 'resolve this record' API](#37-online-resolve-this-record-api) | Real-time Duplicate Check API; Duplicate Check Flow action | T3 | API | None | Jan 2026 | API: Enterprise |
| 38 | [Import-time dedupe](#38-import-time-dedupe) | Transfer (Importr) CSV import | T3 | Import | Partial | Sep 2019 | All plans |
| 39 | [Update rules for incoming matches](#39-update-rules-for-incoming-matches) | Update Rule; Mass Update | T3 | Import | None | Mar 2022 (Mass Update with rules); per-dataset rule Jul 2022 | All plans |
| 40 | [Incremental batch analysis](#40-incremental-batch-analysis) | Incremental analysis default | T3 | Dedupe | None | Oct 2025 | All plans |
| 41 | [Batch ingestion & results API](#41-batch-ingestion--results-api) | Integration (Ingestion) API; Dataset API v2; Records API | T3 | API | None | Oct 2023; v2 May 2025 | Enterprise |
| 42 | [Write cluster IDs back to sources](#42-write-cluster-ids-back-to-sources) | Export with _MasterId; Sync to Duplicate Record Sets | T3 | Dedupe | Partial | Aug 2020 (CSV); Jan 2021 (sync) | Export to SF: Professional+ |
| 43 | [Event webhooks](#43-event-webhooks) | Webhooks (PostMerge, PostConvert) | T3 | API | Partial | Oct 2021 | Not stated (API features listed under Enterprise) |
| 44 | [Pluggable write-back (merge handler)](#44-pluggable-write-back-merge-handler) | Handlers; override merge with Apex or Flow | T3 | API | Partial | Apr 2022; Flow Jan 2026 | Enterprise |
| 45 | [Source attribution of duplicates](#45-source-attribution-of-duplicates) | Insights report | T3 | Assess | Partial | Aug 2026 | Not stated |
| 46 | [Pre-load data integrity check](#46-pre-load-data-integrity-check) | Import Data Integrity Check | T3 | Import | Partial | Sep 2026 | Reporting all plans; fixing in panel Pro+ |
| 47 | [Object-storage ingestion](#47-object-storage-ingestion) | Cloud Storage (S3, SFTP, Salesforce Files); auto import | T3 | Import | Partial | Dec 2023; S3 Sep 2024 | Add-on; included in Enterprise |
| 48 | [AI schema mapping](#48-ai-schema-mapping) | AI CSV field mapping | T3 | Import | None | Nov 2025 | All plans |
| 49 | [Scheduled jobs](#49-scheduled-jobs) | Automate Jobs | T4 | Automate | None | 2019 | Hourly scheduling on pricing grid (tier not readable) |
| 50 | [Decision audit trail](#50-decision-audit-trail) | Log Browser | T4 | Audit | Partial | 2019; before/after Aug 2026 | All plans |
| 51 | [Tag-based approval workflow](#51-tag-based-approval-workflow) | Tags; role-restricted tags; merge only tagged groups | T4 | Dedupe | None | Mar 2023; role tags Apr 2024 | All plans |
| 52 | [Impact estimate before bulk actions](#52-impact-estimate-before-bulk-actions) | Estimated record counts; credit estimates | T4 | Dedupe | None | Apr 2021 | All plans |
| 53 | [Protect records from change](#53-protect-records-from-change) | Protect | T4 | Dedupe | None | Jan 2020 | All plans |
| 54 | [Segment scoping](#54-segment-scoping) | Dataset filters | T4 | Dedupe | Partial | May 2020 | SOQL mode: Professional+ |
| 55 | [Multi-step pipelines](#55-multi-step-pipelines) | Workflows | T4 | Automate | None | Aug 2026 | Enterprise |
| 56 | [Ordered real-time pipelines](#56-ordered-real-time-pipelines) | Triggers | T4 | Automate | Partial | Sep 2022; sequential Jun 2026 | Enrich trigger Pro+ |
| 57 | [Dedupe health dashboard](#57-dedupe-health-dashboard) | Dashboard; Global health KPI; scorecards | T4 | Assess | None | Nov 2020; KPI Jul 2026 | All plans |
| 58 | [Retry, stop and run reports](#58-retry-stop-and-run-reports) | Retry from log; Stop job; email reports | T4 | Audit | None | Stop Aug 2023; retry Nov 2026 | All plans |
| 59 | [Embeddable duplicate widget](#59-embeddable-duplicate-widget) | Duplicates Lightning Component | T4 | Platform | Partial | May 2022; merge Mar 2023 | Lightning-only user $100/yr |
| 60 | [Standardization](#60-standardization) | Format blocks in transform rules | T5 | Cleanse | None | Jun 2021 | All plans |
| 61 | [Transform rule engine](#61-transform-rule-engine) | Transform Rules (Rule Designer) | T5 | Cleanse | None | Sep 2021; regex Aug 2024 | All plans |
| 62 | [Bulk transform, update & delete](#62-bulk-transform-update--delete) | Mass Transform; Mass Update; Mass Delete | T5 | Cleanse | Partial | Jun 2021; preview Mar 2024 | Mass transforms Professional+ |
| 63 | [Record quality scoring](#63-record-quality-scoring) | Data Quality models | T5 | Assess | None | Apr 2024; AI templates Aug 2026 | All plans |
| 64 | [Email verification](#64-email-verification) | Verify: email | T5 | Verify | None | May 2021 | Credits |
| 65 | [Personal vs business email detection](#65-personal-vs-business-email-detection) | Email type detection | T5 | Verify | None | Mar 2026 | Credits |
| 66 | [Phone verification](#66-phone-verification) | Verify: phone | T5 | Verify | None | May 2021; line type Sep 2024 | Credits |
| 67 | [Address verification](#67-address-verification) | Verify: mailing address | T5 | Verify | None | Sep 2021 | Credits |
| 68 | [Email correction & discovery](#68-email-correction--discovery) | Smart email suggestions; email-pattern intelligence | T5 | Verify | None | Jun 2021 | Credits |
| 69 | [Suggestion review queue](#69-suggestion-review-queue) | Review queue (verify & enrich) | T5 | Verify | None | Oct 2026 | Credits |
| 70 | [Verification freshness & caching](#70-verification-freshness--caching) | Stale/changed badges; no re-charge for repeats | T5 | Verify | None | Aug 2025; badges Nov 2026 | Credits |
| 71 | [Website / domain verification](#71-website--domain-verification) | Verify: website | T5 | Verify | None | Sep 2024 | Credits |
| 72 | [Verification API](#72-verification-api) | Verification API; invocable action | T5 | API | Partial | Aug 2024 | Credits |
| 73 | [Push scores back to source](#73-push-scores-back-to-source) | Push DQ / verification scores to Salesforce | T5 | Assess | Partial | Dec 2021; DQ Jun 2024 | All plans |
| 74 | [Role-based access control](#74-role-based-access-control) | Roles (Access, Configuration, Mass Actions) | T6 | Governance | Partial | Sep 2021; granular Nov 2025 | Enterprise |
| 75 | [Security & compliance](#75-security--compliance) | SOC 2 Type II, HIPAA (BAA), GDPR | T6 | Governance | None | SOC 2 2023-24; HIPAA Jan 2024 | All plans |
| 76 | [Environment promotion](#76-environment-promotion) | Copy Configuration between orgs | T6 | Platform | Partial | Nov 2022; full diff Sep 2026 | Standard+ |
| 77 | [Bring your own LLM / AI off switch](#77-bring-your-own-llm--ai-off-switch) | BYO LLM; AI Assistant toggle | T6 | AI | None | Oct 2025; Feb 2026 | All plans |
| 78 | [Service identity for automation](#78-service-identity-for-automation) | Job owner; auto-disable on deactivated users | T6 | Governance | Partial | Sep 2026 | All plans |
| 79 | [Agent / MCP access](#79-agent--mcp-access) | MCP Servers | T6 | API | None | 2026 | Enterprise or trial |
| 80 | [Accessibility](#80-accessibility) | WCAG 2.1 AA | T6 | UX | None | Jul 2026 | All plans |
| 81 | [Conversational agent](#81-conversational-agent) | Oscar AI agent | T7 | AI | None | Sep 2026 | Not stated |
| 82 | [Third-party enrichment orchestration](#82-third-party-enrichment-orchestration) | Enrich (20+ providers, custom MCP) | T7 | Enrich | None | May 2026 | Credits; early preview |
| 83 | [Enrichment staging & triggers](#83-enrichment-staging--triggers) | Staged results; real-time and scheduled enrich | T7 | Enrich | Partial | May-Oct 2026 | Credits |
| 84 | [AI enrichment templates](#84-ai-enrichment-templates) | Quick Start templates (ICP fit, persona, deal risk) | T7 | Enrich | Partial | May 2026 | Credits |
| 85 | [Business-value-at-risk reporting](#85-business-value-at-risk-reporting) | Revenue Exposure (Insights) | T7 | Assess | Partial | Aug 2026 | Not stated |
| 86 | [Pre-send list hygiene](#86-pre-send-list-hygiene) | Campaign verification | T7 | Verify | SF-only | Aug 2026 | Credits |
| 87 | [UX polish](#87-ux-polish) | Themes, folders, keyboard shortcuts, navigation | T7 | UX | None | 2022-2026 | All plans |
| 88 | [Credit & billing mechanics](#88-credit--billing-mechanics) | Flex credits; auto-reload; expiry alerts | T7 | Platform | None | Jun 2021 | All plans |

## Capability detail

### Tier 1: Core matching & resolution

*Without these the product does not dedupe. Every buyer evaluates these first.*

#### 1. Probabilistic / ML pairwise matching

*DataGroomr: Machine Learning matching models · Module: Dedupe · SF dependency: None · First shipped: 2019 core; custom models Nov 2020 · Plan: All plans*

- **What DataGroomr does:** Learned pairwise classifier. Each field comparison adds or subtracts log-odds from a baseline; the total goes through a sigmoid to a 1-100 confidence. Pre-trained models for Leads, Contacts, Accounts; custom models trained per object on the client's own data.
- **Generic capability:** A trainable probabilistic matcher (Fellegi-Sunter with EM, or logistic/GBM on comparison vectors) that works on any table of person records without hand-written rules.
- **Why this rank:** The whole value proposition. Reviewers' most-praised capability is finding duplicates that rule-based tools missed.
- **Source:** <https://datagroomr.com/datagroomr-may26-release-notes/>

#### 2. Clustering pairs into entities

*DataGroomr: Matched duplicate groups · Module: Dedupe · SF dependency: None · First shipped: 2019 core; in-cluster scoring Jan 2026 · Plan: All plans*

- **What DataGroomr does:** Pairs above threshold are linked into groups of any size (no group-size limit). Group confidence is the average of member scores. Classic models can form mixed-confidence groups (Dec 2024); Jan 2026 refined scoring within each cluster.
- **Generic capability:** Transitive clustering (connected components or correlation clustering) that assigns every record a cluster/entity ID. This is literally the 'entity' a unified-profile product sells.
- **Why this rank:** Pairs are not entities. A client buying 'one record per person' is buying the cluster.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923666-using-mass-merge-and-mass-unmatch-to-manage-duplicate-records>

#### 3. Candidate generation (blocking) at scale

*DataGroomr: Implicit blocking; 'broader blocking' in Similar v2 · Module: Dedupe · SF dependency: None · First shipped: Core; widened Jan 2026 · Plan: All plans*

- **What DataGroomr does:** Not configurable or documented in-product. Early blog posts describe learned predicate and index blocking (n-grams, tokens). Jan 2026 widened blocking. Each analysis run has a hard 3-hour limit; the fix is to split datasets with filters or raise the confidence floor.
- **Generic capability:** Configurable and learned blocking keys, LSH/MinHash, sorted-neighbourhood; pair-count estimates before a run. Determines both recall and compute cost on 10M+ rows.
- **Why this rank:** Invisible to users but decides cost and recall. DataGroomr's 3-hour cap and 'split your dataset' advice show where it strains. Your DuckDB-on-K8s cost story lives here.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001924094-how-long-does-it-take-to-analyze-dataset->

#### 4. Deterministic & fuzzy rule matching

*DataGroomr: Classic matching models (Exact / Similar) · Module: Dedupe · SF dependency: None · First shipped: Nov 2022; fuzzy Dec 2024 · Plan: All plans*

- **What DataGroomr does:** Weighted sum of field results: Exact adds full weight, Similar adds a partial score, mismatches are penalized. Fuzzy uses edit-distance, phonetic and token-overlap. Exact-only at launch (Nov 2022), fuzzy 'Similar' Dec 2024, Similar v2 Jan 2026, text threshold default 65% (Nov 2026).
- **Generic capability:** Rule-based matcher with exact, Jaro-Winkler/Levenshtein, phonetic (Soundex/Metaphone) and token comparators, field weights and a threshold. Runs with zero labels.
- **Why this rank:** Cold start before labels exist, regulated clients who need deterministic logic, and migrating a client's existing rules.
- **Source:** <https://datagroomr.com/datagroomr-may26-release-notes/>

#### 5. Calibrated confidence & threshold bands

*DataGroomr: Match Confidence; Minimum Match Confidence; dual slider · Module: Dedupe · SF dependency: None · First shipped: May 2020; dual slider May 2025 · Plan: All plans*

- **What DataGroomr does:** Every group gets a 1-100 score. Each dataset sets a minimum confidence; each extra model has its own slider. Mass actions take a range (e.g. merge only 80-95%) since May 2025.
- **Generic capability:** Three-band policy: auto-accept above X, human review between Y and X, reject below Y. Bands can differ per entity type or source.
- **Why this rank:** The control clients use to trade automation for risk. Without it they cannot turn on auto-merge.
- **Source:** <https://datagroomr.com/datagroomr-may25-release-notes/>

#### 6. Survivor (master record) selection rules

*DataGroomr: Master Record Rules · Module: Dedupe · SF dependency: None · First shipped: Sep 2020 · Plan: All plans*

- **What DataGroomr does:** Block editor: SORT GROUP BY a field, THEN SORT tie-breakers, optional BUT PRIORITIZE RECORD IF conditions (regex, AND/OR, match confidence), then pick first or last. Default rules since Sep 2020: most relevant (highest score), most recently modified, oldest; a 'Most Related Records' template was added Nov 2026. AI can draft rules.
- **Generic capability:** Golden-record anchor policy: which source record's identity wins (trusted source, most complete, most recent, most activity).
- **Why this rank:** Dedupe without a survivor policy produces a list, not clean data. Survivorship config is a top setup complaint, so ease here wins deals.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002054856-merge-rules-master-record-selection>

#### 7. Attribute-level survivorship

*DataGroomr: Field Merge Rules (Field Value Rules) · Module: Dedupe · SF dependency: None · First shipped: Oct 2020; distribute Apr 2021; advanced logic Nov 2025 · Plan: Advanced rules: Professional+*

- **What DataGroomr does:** Per field: value from newest/oldest/highest record, most common/highest/lowest/average/sum in group, concatenate unique values, distribute extra values into empty fields (e.g. 3 phones into Phone, Mobile, Other), keep TRUE if any is TRUE (opt-outs), substring, regex conditions. Default rule fills empty master fields from the most recently modified record.
- **Generic capability:** Per-attribute survivorship with provenance, computed into a golden-record view rather than overwriting sources.
- **Why this rank:** Golden-record quality is decided field by field. The 'keep opt-out TRUE' pattern is a compliance must for any person data.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002113237-field-merge-rule>

#### 8. Human review queue (side-by-side)

*DataGroomr: Review dialog · Module: Dedupe · SF dependency: None · First shipped: 2019 core · Plan: All plans*

- **What DataGroomr does:** Group shown side by side with differences in red. Swap master, click to copy a value into the master, edit inline, show/hide empty and identical fields, show related-record counts, show verification status and AI recommendation.
- **Generic capability:** Clerical review UI for the gray-zone band, with record context, keyboard flow and bulk decisions.
- **Why this rank:** Gray-zone pairs always exist; review throughput sets time-to-value. One reviewer had to open the CRM to understand a match, which shows what's missing.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923661-working-with-duplicate-records-matched-groups->

#### 9. Bulk resolution of clusters

*DataGroomr: Mass Merge · Module: Dedupe · SF dependency: None · First shipped: 2019 core; filters May 2025; parallel Nov 2026 · Plan: All plans (trial capped at 25 merged records)*

- **What DataGroomr does:** Merge every group matching filters: confidence range, tags in/out, group size, which model matched, which field matched. Shows estimated record count first. Nov 2026: parallel batches that back off on row locks and rate limits, about 2-3x faster.
- **Generic capability:** Batch apply survivorship to thousands of clusters, with adaptive throttling against the destination system.
- **Why this rank:** Backlogs run to millions of records. Reviewers cite mass merge as a top time saver.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923666-using-mass-merge-and-mass-unmatch-to-manage-duplicate-records>

#### 10. Re-point related records to the survivor

*DataGroomr: Merge with re-parenting; related-record handling · Module: Dedupe · SF dependency: Partial · First shipped: 2019 core; edge cleanup Nov 2020 · Plan: All plans*

- **What DataGroomr does:** Merging moves child records (opportunities, cases, tasks, notes, custom children) to the master. Collapses redundant relationships automatically (contact-to-account links, roles unioned). Oct 2026 fix: custom-object merges no longer delete a child record unless it can be re-created on the master.
- **Generic capability:** When clusters resolve, foreign keys in every dependent table point to the surviving entity ID; duplicate edges collapse. In a non-destructive model this is a crosswalk table, not a rewrite.
- **Why this rank:** A merge that orphans orders or tickets destroys trust. Applies to any relational source, not just CRMs.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002231624-merge-contacts-with-multiple-accounts-records>

#### 11. Reject decisions remembered (must-not-link)

*DataGroomr: Unmatch; Mass Unmatch; Undo Unmatch · Module: Dedupe · SF dependency: None · First shipped: 2019; mass Nov 2025 · Plan: Undo Unmatch: Enterprise*

- **What DataGroomr does:** Unmatch ungroups records and prevents them regrouping in future runs; decisions also feed ML learning. Mass Unmatch (Nov 2025) uses the same filters as Mass Merge. Undo Unmatch is Enterprise.
- **Generic capability:** Persistent cannot-link constraints that survive re-runs and model changes, plus bulk split.
- **Why this rank:** Correcting clusters matters as much as forming them. A false pair that keeps coming back kills reviewer trust.
- **Source:** <https://datagroomr.com/datagroomr-november25-release-notes/>

#### 12. Undo a merge

*DataGroomr: Undo Merge; Undo All · Module: Audit · SF dependency: Partial · First shipped: Sep 2021; Undo All Aug 2023 · Plan: Add-on; included in Enterprise*

- **What DataGroomr does:** Restores deleted duplicates and re-links their children, but only while records sit in the Salesforce Recycle Bin (typically 15 days). Does not restore overwritten master field values. Undo All covers a whole mass-merge job (Aug 2023).
- **Generic capability:** Reversible resolution: soft-merge with an event log, so any link can be undone at any time.
- **Why this rank:** Trust is what lets buyers enable auto-merge. Third parties criticise that this safety net is a paid add-on.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002265857-undo-and-rollback-a-merge->

#### 13. Full rollback including field values

*DataGroomr: Rollback Merge · Module: Audit · SF dependency: Partial · First shipped: Dec 2023 / Jan 2024 · Plan: Separate add-on*

- **What DataGroomr does:** Takes a backup before each merge and stores it in the customer's Salesforce Files. No time limit while files exist. Restores children and field values on both master and duplicates.
- **Generic capability:** Versioned entity store with time travel: revert any cluster to any prior state, including survivorship output.
- **Why this rank:** Undo alone leaves overwritten values lost. DataGroomr's own press release claimed only one competitor had anything similar.
- **Source:** <https://datagroomr.com/winter-24-updates-undo-merges-indefinitely-cloud-storage-support/>

#### 14. Cross-source matching

*DataGroomr: Cross-object datasets · Module: Dedupe · SF dependency: Partial · First shipped: Jun 2020; any object May 2022 · Plan: Professional+*

- **What DataGroomr does:** Matches one object against another (Leads vs Contacts, any object vs any object since May 2022) with a field-mapping step; unmapped fields are not compared. The parent side always survives. Since Feb 2026 can be built straight from objects.
- **Generic capability:** Match records that live in different tables or systems (CRM contacts vs billing customers vs support users) through a schema mapping. The core of entity resolution.
- **Why this rank:** The same person exists in several places. Gating it to a higher tier is a pricing lever, but for a unified-profile buyer it is core.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001988800-find-duplicates-across-objects-or-datasets>

#### 15. Pre-match normalization

*DataGroomr: Basic preprocessing; name, company, address parsing · Module: Dedupe · SF dependency: None · First shipped: Jul 2020; opt-out May 2026 · Plan: All plans*

- **What DataGroomr does:** Before any comparison: lowercase, trim, strip special characters; phones to digits; emails lowercased; websites stripped of protocol and www. Company names, addresses and surnames parsed automatically (Jul 2020). Basic cleanup can be switched off for SKUs and codes (May 2026).
- **Generic capability:** Canonicalize every match field before comparison, with per-field opt-out.
- **Why this rank:** Cheap, invisible and a large share of recall.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002689406-how-do-i-match-across-different-fields-in-a-matching-model->

#### 16. Synonym, nickname & stop-word dictionaries

*DataGroomr: Dictionaries (Synonym / Ignore Words) · Module: Dedupe · SF dependency: None · First shipped: Jan 2025 · Plan: Professional+*

- **What DataGroomr does:** Each dictionary is assigned as Synonym or Ignore Words in Classic and ML models. Built-in Company Suffixes, Western Names and street abbreviations; custom dictionaries by CSV. Also usable in transforms.
- **Generic capability:** Nickname tables (Bob=Robert), company suffix stripping (Inc, LLC, GmbH), address abbreviations, client-specific aliases.
- **Why this rank:** Person matching lives and dies on nicknames. Low cost, high recall gain.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002578165-dictionaries>

### Tier 2: Accuracy, tuning & trust

*Separates a tool that finds duplicates from one a client will let auto-merge.*

#### 17. Per-pair match explanation

*DataGroomr: Match Confidence transparency · Module: Dedupe · SF dependency: None · First shipped: May 2026 · Plan: All plans*

- **What DataGroomr does:** Clicking the chart icon next to a score shows the top 3 contributing fields; full panel shows each field's match strength, weight and impact (Classic) or log-odds contribution and sigmoid conversion (ML), plus the preprocessing applied. Negative evidence shown in red.
- **Generic capability:** Why-matched / why-not attributions on every pair and cluster, exportable for audit.
- **Why this rank:** Reviewers and auditors need it before trusting automation. DataGroomr only added it in 2026; Senzing and Splink set the bar.
- **Source:** <https://datagroomr.com/datagroomr-may26-release-notes/>

#### 18. Hard match constraints

*DataGroomr: Required match (Classic); Required Field (ML) · Module: Dedupe · SF dependency: None · First shipped: Jun 2025; ML Aug 2025 · Plan: All plans*

- **What DataGroomr does:** A field such as Tax ID or account number must match exactly and be non-blank before similarity counts.
- **Generic capability:** Must-match keys and veto rules (different SSN or DOB = never the same person) layered over the probabilistic score.
- **Why this rank:** Prevents the catastrophic false merge on strong identifiers.
- **Source:** <https://datagroomr.com/datagroomr-june25-release-notes/>

#### 19. Active-learning labeling

*DataGroomr: Train manually (Yes / No / Not Sure); Coverage metric · Module: Dedupe · SF dependency: None · First shipped: Nov 2020; coverage Aug 2024 · Plan: All plans*

- **What DataGroomr does:** Labeler shows candidate pairs; recommended labels are about 5 positive and 5 negative per field. A Coverage score shows how much of the data the model judges confidently. Continuous training adds new answers instead of retraining from scratch.
- **Generic capability:** Uncertainty-sampled labeling UI with a convergence metric, so a client trains a model in an hour.
- **Why this rank:** Turns client knowledge into model accuracy cheaply. A minimum label count gate is not documented.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002127041-training-machine-learning-matching-model>

#### 20. Learning from reviewer decisions

*DataGroomr: Auto training · Module: Dedupe · SF dependency: None · First shipped: Nov 2020 · Plan: All plans*

- **What DataGroomr does:** When on, the model retrains periodically based on users' merge and unmatch actions.
- **Generic capability:** Implicit-feedback loop: every accept/reject in review becomes training data.
- **Why this rank:** Accuracy compounds with use; reviewers say the tool 'needs less management over time'.
- **Source:** <https://datagroomr.com/november-20-release-whats-new-in-datagroomr/>

#### 21. Configurable field weights

*DataGroomr: Classic field weights · Module: Dedupe · SF dependency: None · First shipped: Aug 2023 · Plan: All plans*

- **What DataGroomr does:** Default weight is 100/N per field; configurable since Jul/Aug 2023. Nov 2026 editor forces weights to total 100% with a 'Balance to 100%' button.
- **Generic capability:** Per-field weights (or learned m/u probabilities) that the client can inspect and override.
- **Why this rank:** Lets admins encode domain knowledge (last name matters more than city).
- **Source:** <https://datagroomr.com/august-23-updates-performance-and-ui-improvements-mass-undo-and-more/>

#### 22. Null / blank handling policy

*DataGroomr: Blank value options · Module: Dedupe · SF dependency: None · First shipped: Sep 2024 (blank matching); three options Dec 2024 · Plan: All plans*

- **What DataGroomr does:** Per field: match if both blank (default), match if either blank, or don't match on blanks. Analyst note: with exact keys, the 'both blank' default can lump unrelated records with empty keys into one group.
- **Generic capability:** Explicit missing-value semantics per field; probabilistic models should treat nulls as no evidence.
- **Why this rank:** The default 'both blank = match' is a known false-positive trap. Getting nulls right is a precision win.
- **Source:** <https://datagroomr.com/datagroomr-december24-release-notes/>

#### 23. Cross-field comparison

*DataGroomr: Field groups; Cross Compare Fields · Module: Dedupe · SF dependency: None · First shipped: Aug 2024 · Plan: All plans*

- **What DataGroomr does:** Fields sharing a group name are compared both ways (work, home, mobile phones; billing vs shipping state), catching values in the wrong field.
- **Generic capability:** Compare any value of a type against any field of that type (phone-in-any-phone-field, email-in-any-email-field).
- **Why this rank:** Real records put data in the wrong column constantly.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002689406-how-do-i-match-across-different-fields-in-a-matching-model->

#### 24. Derived match keys (pre-match transforms)

*DataGroomr: Transform rule in preprocessing; First N characters · Module: Dedupe · SF dependency: None · First shipped: First N Aug 2024; transforms May 2025 · Plan: Pre-match transforms on pricing grid (tier not readable)*

- **What DataGroomr does:** Run a full transform before scoring (email to domain, trim prefixes). Compare only the first N characters (loose names, phone before extension).
- **Generic capability:** Feature engineering per field: derived keys, substrings, phonetic codes, domain extraction.
- **Why this rank:** Lets one engine handle messy, client-specific formats without code.
- **Source:** <https://datagroomr.com/datagroomr-may25-release-notes/>

#### 25. Type-aware similarity (dates, numbers, geo, address)

*DataGroomr: Similar for dates, numbers, locations; whole address fields · Module: Dedupe · SF dependency: None · First shipped: Dec 2025; Nov 2026 · Plan: All plans*

- **What DataGroomr does:** Nov 2026: Similar works on date, number, currency, percent and geolocation fields with a ± tolerance (days, minutes, radius). Compound address fields can be matched whole (Dec 2025).
- **Generic capability:** Comparators for DOB with transposition tolerance, numeric closeness, geo distance, parsed-address components.
- **Why this rank:** DOB and address are core person keys; string distance alone handles them badly.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002864844-november-2026>

#### 26. Features from related records

*DataGroomr: Related-object fields in models (up to 2 hops) · Module: Dedupe · SF dependency: Partial · First shipped: Jan 2024; 2 hops Jul 2026 · Plan: All plans*

- **What DataGroomr does:** Contacts can match using parent Account fields; references up to two relationships deep (Contact → Account → Owner) since Jul 2026. Child values still need a workaround.
- **Generic capability:** Pull features from joined tables (household address, employer, account) into the comparison vector.
- **Why this rank:** Context outside the row often decides a match.
- **Source:** <https://datagroomr.com/datagroomr-summer26-release-notes/>

#### 27. LLM adjudication of gray-zone groups

*DataGroomr: AI Recommendations (merge, manual merge, unmatch, link, split) · Module: AI · SF dependency: None · First shipped: Sep 2025 · Plan: Credits (free with BYO LLM)*

- **What DataGroomr does:** An LLM reviews each group and recommends an action with reasoning, tagging the group. Works one by one, in bulk, or automatically on new Live Dedupe matches. Runs on Claude Sonnet 4.5 since Jan 2026. Uses credits unless the client brings its own LLM.
- **Generic capability:** LLM reviewer for the uncertain band that drafts decisions and reasons for humans to approve. The LLM does not do the matching.
- **Why this rank:** Cuts reviewer time on the gray zone. 'Split' and 'link' are useful entity-level corrections.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002686885-get-ai-recommendations-for-duplicate-resolution>

#### 28. Zero-config starter models

*DataGroomr: Pre-trained models; AI-built models at sign-up · Module: Dedupe · SF dependency: None · First shipped: Oct 2020; AI onboarding Mar 2025 · Plan: All plans*

- **What DataGroomr does:** Pre-trained models run on first login (Oct 2020). Since Mar 2025 AI profiles the org at sign-up and builds custom matching models (consistency DQ models are also auto-created from a format scan); Aug 2025 auto-trains ML on the client's data and notifies the user when ready.
- **Generic capability:** Sensible default matcher per entity type (person, company, address) that produces results within minutes of connecting data.
- **Why this rank:** Time-to-first-value is how trials convert. Ease of setup is DataGroomr's highest-rated attribute on G2.
- **Source:** <https://datagroomr.com/datagroomr-march25-release-notes/>

#### 29. Natural-language rule authoring

*DataGroomr: Design with AI; Explain Rule; AI-generated rules · Module: AI · SF dependency: None · First shipped: May 2025; v2 Nov 2025; Explain Dec 2025 · Plan: All plans*

- **What DataGroomr does:** Describe a master, field-merge, transform or DQ rule in plain English and get a block configuration; 'Explain Rule' turns existing logic into prose; AI drafts merge rules from metadata (completeness, recency, verification fields).
- **Generic capability:** Prompt-to-rule and rule-to-prose for survivorship and matching config.
- **Why this rank:** The #1 complaint across reviews is the rules learning curve. This is the direct fix.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002638752-ai-assistant-in-rule-designer>

#### 30. Rule preview on real records

*DataGroomr: Rule Preview with what-if editing · Module: Dedupe · SF dependency: None · First shipped: Apr 2022; what-if Aug 2026 · Plan: All plans*

- **What DataGroomr does:** Preview a rule against sample or live records (random, per row, or search by ID); edit cells inline to test scenarios; results refresh automatically.
- **Generic capability:** Dry-run survivorship and match rules on real clusters before applying.
- **Why this rank:** Lowers the fear of bulk merges; makes rule debugging fast.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002119357-rule-preview>

#### 31. Ensembles & cascades of models

*DataGroomr: Multi-model matching (up to 3) · Module: Dedupe · SF dependency: None · First shipped: Apr 2025 · Plan: Enterprise*

- **What DataGroomr does:** Up to 3 models per dataset, each with its own threshold. 'Then Run' cascades (later models see only unmatched records); 'And Run' runs in parallel and merges results. Color badges show which model matched.
- **Generic capability:** Cascade a high-precision deterministic pass before a probabilistic pass; union results per entity type.
- **Why this rank:** Different duplicate patterns need different logic. Cap of 3 is an obvious ceiling to beat.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002125898-configuring-dedupe-datasets>

#### 32. Relationship exclusions

*DataGroomr: Exclude same-hierarchy accounts · Module: Dedupe · SF dependency: Partial · First shipped: Dec 2025 · Plan: All plans*

- **What DataGroomr does:** On by default: accounts in the same parent/child hierarchy never match each other. Can be turned on explicitly.
- **Generic capability:** Must-not-link from known relationships (parent vs subsidiary, family members sharing an address, joint accounts).
- **Why this rank:** Family members at one address are the classic false positive in person ER.
- **Source:** <https://datagroomr.com/datagroomr-december25-release-notes/>

#### 33. Household / group entities

*DataGroomr: AI 'Link' recommendation; NPSP household handling · Module: Dedupe · SF dependency: Partial · First shipped: Sep 2025 (link); NPSP Apr 2025; households Aug 2026 · Plan: NPSP merge: Professional+*

- **What DataGroomr does:** AI can recommend 'link' (parent/child, household) instead of merge. NPSP-aware contact merger (Apr 2025; Pro toggle Jun 2025); re-parenting of household accounts and affiliations added Aug 2026. No first-class household entity outside Salesforce NPSP.
- **Generic capability:** Second-level entities (household, company) linking person entities without merging them.
- **Why this rank:** Valuable for unified views (who lives with whom) but secondary to person-level dedupe.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002686885-get-ai-recommendations-for-duplicate-resolution>

#### 34. Model versioning & A/B

*DataGroomr: Model versions; Similar v1 pin; clone across types · Module: Dedupe · SF dependency: None · First shipped: Oct 2025; pin Jan 2026 · Plan: All plans*

- **What DataGroomr does:** ML models cannot be edited, only cloned and retrained; versions selectable per dataset. Existing models stay on Similar v1 when v2 shipped. Clone ML to Classic and back to compare on the same data (Oct 2025).
- **Generic capability:** Immutable, versioned matcher configs with side-by-side comparison of cluster outputs.
- **Why this rank:** Clients need stable results; silent re-clustering breaks downstream IDs.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002125863-manage-matching-models>

#### 35. AI-assisted model setup & training

*DataGroomr: AI field selection; Train with AI · Module: AI · SF dependency: None · First shipped: Mar 2025; Apr 2025 · Plan: All plans*

- **What DataGroomr does:** AI picks fields from metadata and data distribution. 'Train with AI' has an LLM label pairs for the ML model (Apr 2025).
- **Generic capability:** LLM-as-labeler and automated field selection to bootstrap a model without a data scientist.
- **Why this rank:** Shrinks setup further, but label quality from an LLM needs validation.
- **Source:** <https://datagroomr.com/datagroomr-april25-release-notes/>

### Tier 3: Prevention & integration

*Stops duplicates from coming back and lets the engine sit inside the client's systems.*

#### 36. Real-time incremental matching

*DataGroomr: Live Dedupe · Module: Dedupe · SF dependency: Partial · First shipped: Aug 2025; GA Dec 2025 · Plan: Pro 1 dataset, Enterprise 3; $500/yr per extra*

- **What DataGroomr does:** Watches create, update, delete and undelete events (Salesforce CDC) and matches only changed records against a persisted index that rebuilds when the model changes. Flag for review or auto-merge above a threshold. Cross-object since Jul 2026.
- **Generic capability:** Streaming entity resolution off CDC (Debezium, Kafka, Postgres logical replication, warehouse streams) against a persisted blocking index.
- **Why this rank:** Keeps the problem from coming back. Roundups now treat real-time prevention as expected.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002779873-live-dedupe-deduplication-in-real-time>

#### 37. Online 'resolve this record' API

*DataGroomr: Real-time Duplicate Check API; Duplicate Check Flow action · Module: API · SF dependency: None · First shipped: Jan 2026 · Plan: API: Enterprise*

- **What DataGroomr does:** POST a record and get back confidence scores and the likely master from the Live Dedupe index, before it is saved. A Flow action blocks duplicates inside Salesforce automations (Feb 2026).
- **Generic capability:** Low-latency /resolve endpoint returning candidate entity IDs and scores, for forms, ingestion pipelines and middleware.
- **Why this rank:** How the engine embeds in client systems; table stakes for a DaaS offering.
- **Source:** <https://datagroomr.com/datagroomr-january26-release-notes/>

#### 38. Import-time dedupe

*DataGroomr: Transfer (Importr) CSV import · Module: Import · SF dependency: Partial · First shipped: Sep 2019 · Plan: All plans*

- **What DataGroomr does:** A file is matched against existing records before insert and sorted into Unmatched, Matched, Imported and Mass Processed. Unique rows import; matches update existing records instead. Staged data kept 30 days. Can compare against two objects at once but loads one at a time.
- **Generic capability:** Screen any incoming batch (file, list purchase, migration) against the entity store; insert, link or update.
- **Why this rank:** The cheapest place to stop a duplicate. Reviewers praise it.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923676-import-data-from-csv-files>

#### 39. Update rules for incoming matches

*DataGroomr: Update Rule; Mass Update · Module: Import · SF dependency: None · First shipped: Mar 2022 (Mass Update with rules); per-dataset rule Jul 2022 · Plan: All plans*

- **What DataGroomr does:** For matched rows: update empty fields from file, update all fields, update nothing, or a custom merge rule; bounded by a confidence range.
- **Generic capability:** Survivorship applied to incoming data, not just existing clusters.
- **Why this rank:** Import dedupe is only half done if matched rows can't enrich the existing entity safely.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002311264-mass-update-matched-salesforce-records>

#### 40. Incremental batch analysis

*DataGroomr: Incremental analysis default · Module: Dedupe · SF dependency: None · First shipped: Oct 2025 · Plan: All plans*

- **What DataGroomr does:** The Analyze button runs incremental analysis by default; full analysis sits under a menu.
- **Generic capability:** Re-score only new and changed records and their candidate neighbours between full rebuilds.
- **Why this rank:** Major compute saving on large datasets; directly relevant to cost.
- **Source:** <https://datagroomr.com/datagroomr-octber25-release-notes/>

#### 41. Batch ingestion & results API

*DataGroomr: Integration (Ingestion) API; Dataset API v2; Records API · Module: API · SF dependency: None · First shipped: Oct 2023; v2 May 2025 · Plan: Enterprise*

- **What DataGroomr does:** Create datasets, push records, run analysis, imports, updates and verification; list matched and unmatched records. v2 pages 1,000 records at a time. Limit 60 requests per minute per org; key in a token header.
- **Generic capability:** Bulk push-and-pull API for match jobs and cluster results.
- **Why this rank:** Required for any pipeline integration. 60 rpm is low for a data service.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002490152-integration-api-and-secret-keys>

#### 42. Write cluster IDs back to sources

*DataGroomr: Export with _MasterId; Sync to Duplicate Record Sets · Module: Dedupe · SF dependency: Partial · First shipped: Aug 2020 (CSV); Jan 2021 (sync) · Plan: Export to SF: Professional+*

- **What DataGroomr does:** Exports carry _IsMaster and _MasterId columns. Groups above a confidence can be written into Salesforce's native duplicate objects for in-CRM reporting and approval.
- **Generic capability:** Write entity_id (or a crosswalk) into every source system so downstream tools join on it.
- **Why this rank:** For a unified view, the ID must reach every system; destructive merge is optional.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002145472-exporting-records-in-dedupe>

#### 43. Event webhooks

*DataGroomr: Webhooks (PostMerge, PostConvert) · Module: API · SF dependency: Partial · First shipped: Oct 2021 · Plan: Not stated (API features listed under Enterprise)*

- **What DataGroomr does:** POSTs merge and convert events with request and result payloads; signed with HMAC-SHA256 and a correlation ID header. Only two event types; no documented retry policy.
- **Generic capability:** Signed events for every link, unlink, merge and survivorship change.
- **Why this rank:** Lets downstream systems react to entity changes. DataGroomr's event coverage is thin.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002274608-webhooks>

#### 44. Pluggable write-back (merge handler)

*DataGroomr: Handlers; override merge with Apex or Flow · Module: API · SF dependency: Partial · First shipped: Apr 2022; Flow Jan 2026 · Plan: Enterprise*

- **What DataGroomr does:** Replace the native merge with a call to the client's endpoint or a Salesforce Flow, which receives master and duplicate IDs and returns results for the audit log.
- **Generic capability:** Adapter interface so each destination decides how to apply a resolution (merge, link, flag, write crosswalk).
- **Why this rank:** Every client's systems have side effects; a handler keeps the core engine generic.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002492800-handlers>

#### 45. Source attribution of duplicates

*DataGroomr: Insights report · Module: Assess · SF dependency: Partial · First shipped: Aug 2026 · Plan: Not stated*

- **What DataGroomr does:** Duplicate creation rate, which lead source, user or integration creates duplicates, recurrence rate (merged groups that come back within N days), pattern distribution, merge velocity, confidence distribution, model coverage gaps.
- **Generic capability:** Root-cause analytics per source system, so the client fixes the leaking pipeline.
- **Why this rank:** Tells the client which upstream system to fix; turns a cleanup tool into an ongoing service.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002823300-insights-report>

#### 46. Pre-load data integrity check

*DataGroomr: Import Data Integrity Check · Module: Import · SF dependency: Partial · First shipped: Sep 2026 · Plan: Reporting all plans; fixing in panel Pro+*

- **What DataGroomr does:** Ten checks on staged rows: required fields, lookup references (accept all ≥85% similarity), picklists, booleans, types, max length, formats, uniqueness, writability. Blocking, warning and info levels.
- **Generic capability:** Schema/contract validation of incoming batches before they reach the entity store.
- **Why this rank:** Stops bad loads, but secondary to matching.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002848746-import-data-integrity-check>

#### 47. Object-storage ingestion

*DataGroomr: Cloud Storage (S3, SFTP, Salesforce Files); auto import · Module: Import · SF dependency: Partial · First shipped: Dec 2023; S3 Sep 2024 · Plan: Add-on; included in Enterprise*

- **What DataGroomr does:** Connect S3 or SFTP; watch a folder and auto-import or auto-update new files without creating duplicates.
- **Generic capability:** Pull from S3/GCS/SFTP/Parquet drops on a schedule or event.
- **Why this rank:** Natural fit for a warehouse-native service; DataGroomr charges extra for it.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002494826-upload-csv-file-or-folder-from-local-device-or-cloud-storage>

#### 48. AI schema mapping

*DataGroomr: AI CSV field mapping · Module: Import · SF dependency: None · First shipped: Nov 2025 · Plan: All plans*

- **What DataGroomr does:** Auto-maps file columns to target fields; unmapped columns are kept for analysis and export.
- **Generic capability:** Automatic column-to-attribute mapping for each new source.
- **Why this rank:** Speeds onboarding of new sources.
- **Source:** <https://datagroomr.com/datagroomr-november25-release-notes/>

### Tier 4: Operating at scale

*Automation, audit and safety rails for running dedupe as a standing process.*

#### 49. Scheduled jobs

*DataGroomr: Automate Jobs · Module: Automate · SF dependency: None · First shipped: 2019 · Plan: Hourly scheduling on pricing grid (tier not readable)*

- **What DataGroomr does:** Ten job types (analyze, mass merge, convert, sync, transform, verify, enrich, delete, import, update) targeted by confidence, tag, matched field, model or group size. One-time, hourly, daily, weekly, monthly; API-triggerable. Jobs run as their owner.
- **Generic capability:** Orchestrated batch runs (cron, Argo, Airflow) with targeting filters.
- **Why this rank:** Dedupe is a standing process, not a one-off.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923682-jobs-task-scheduling>

#### 50. Decision audit trail

*DataGroomr: Log Browser · Module: Audit · SF dependency: Partial · First shipped: 2019; before/after Aug 2026 · Plan: All plans*

- **What DataGroomr does:** 17 event types with who, when, which dataset or job; re-parented record IDs; before/after field values (requires Salesforce field history); AI suggestions and reasoning logged; export to CSV. Logs kept 12 months.
- **Generic capability:** Immutable lineage of every match, merge, override and AI suggestion, queryable per entity.
- **Why this rank:** Compliance and debugging. Field-level history should not depend on a source system feature.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001926969-log-browser>

#### 51. Tag-based approval workflow

*DataGroomr: Tags; role-restricted tags; merge only tagged groups · Module: Dedupe · SF dependency: None · First shipped: Mar 2023; role tags Apr 2024 · Plan: All plans*

- **What DataGroomr does:** Tag groups (manually or by AI), restrict which roles can apply which tags, and schedule merges that only act on approved tags. A shared tag library syncs into the CRM.
- **Generic capability:** Steward approval states on clusters (approved, rejected, needs info) that drive automation.
- **Why this rank:** Lets a business user approve and an engine execute.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002399355-organizing-and-automating-with-tags-in-dedupe>

#### 52. Impact estimate before bulk actions

*DataGroomr: Estimated record counts; credit estimates · Module: Dedupe · SF dependency: None · First shipped: Apr 2021 · Plan: All plans*

- **What DataGroomr does:** Mass merge, convert and update show how many records fall in the selection before running; credit costs shown before verification and enrichment.
- **Generic capability:** Preview counts and cost for any bulk operation.
- **Why this rank:** Cheap to build, big for confidence.
- **Source:** <https://datagroomr.com/april-21-release-new-merge-rules-and-ui-enhancements/>

#### 53. Protect records from change

*DataGroomr: Protect · Module: Dedupe · SF dependency: None · First shipped: Jan 2020 · Plan: All plans*

- **What DataGroomr does:** Protected records are never modified or deleted.
- **Generic capability:** Do-not-merge flags (VIP, legal hold, regulated accounts).
- **Why this rank:** Small feature, prevents expensive mistakes.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44001923661-working-with-duplicate-records-matched-groups->

#### 54. Segment scoping

*DataGroomr: Dataset filters · Module: Dedupe · SF dependency: Partial · First shipped: May 2020 · Plan: SOQL mode: Professional+*

- **What DataGroomr does:** Visual filter builder or SOQL; relative dates (Apr 2026), IN/NOT IN, filters on related records, filter by matched field. Unlimited datasets per org.
- **Generic capability:** SQL predicates to scope a run (region, source, date range) and partition big jobs.
- **Why this rank:** Needed to run safely by segment; also a manual blocking strategy.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002125898-configuring-dedupe-datasets>

#### 55. Multi-step pipelines

*DataGroomr: Workflows · Module: Automate · SF dependency: None · First shipped: Aug 2026 · Plan: Enterprise*

- **What DataGroomr does:** 2-10 steps in order (analyze, merge, convert, sync, transform, verify, enrich, delete, import, update) with Wait steps (1 min-24 h), stop on failure unless 'continue on error', one schedule, API trigger, completion email.
- **Generic capability:** DAG orchestration of normalize → match → review → resolve → write-back.
- **Why this rank:** Useful for operations teams; a data service would expose this as its own pipeline.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002823250-workflows-multi-step-automation-pipelines>

#### 56. Ordered real-time pipelines

*DataGroomr: Triggers · Module: Automate · SF dependency: Partial · First shipped: Sep 2022; sequential Jun 2026 · Plan: Enrich trigger Pro+*

- **What DataGroomr does:** Per-object ordered pipeline on create or update: dedupe (Classic models only), transform, verify, enrich. Runs sequentially; a failed step is logged and the rest continue; a second change waits.
- **Generic capability:** Ordered stream processors on CDC: normalize, verify, then match.
- **Why this rank:** Normalizing before real-time matching improves accuracy.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002375030-triggers-real-time-actions>

#### 57. Dedupe health dashboard

*DataGroomr: Dashboard; Global health KPI; scorecards · Module: Assess · SF dependency: None · First shipped: Nov 2020; KPI Jul 2026 · Plan: All plans*

- **What DataGroomr does:** Duplicate rate, incomplete records, unverified contacts, overall DQ score with trends; per-object scorecards; duplicate and merge history widgets.
- **Generic capability:** Duplicate rate, cluster-size distribution, merge velocity and backlog per source.
- **Why this rank:** Shows ROI; renews contracts.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002543811-data-quality-dashboard>

#### 58. Retry, stop and run reports

*DataGroomr: Retry from log; Stop job; email reports · Module: Audit · SF dependency: None · First shipped: Stop Aug 2023; retry Nov 2026 · Plan: All plans*

- **What DataGroomr does:** Re-run failed imports, merges, converts, updates, transforms and deletes from the log (Nov 2026). Stop a running job (operations already sent may still finish). Emailed run reports with charts.
- **Generic capability:** Idempotent retries, cancellation and run summaries.
- **Why this rank:** Operational hygiene for long jobs.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002864844-november-2026>

#### 59. Embeddable duplicate widget

*DataGroomr: Duplicates Lightning Component · Module: Platform · SF dependency: Partial · First shipped: May 2022; merge Mar 2023 · Plan: Lightning-only user $100/yr*

- **What DataGroomr does:** Shows a record's duplicates on its page; business users can merge, tag or unmatch without a full seat ('Lightning-only' users, $100/user/yr). Updates live.
- **Generic capability:** Embeddable 'possible duplicates' panel for any host app (CRM, support desk, admin portal).
- **Why this rank:** Spreads review to front-line users, but optional for a data service.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002413768-using-duplicates-lightning-component>

### Tier 5: Input hygiene (cleanse & verify)

*Adjacent features whose main value to a dedupe buyer is better match keys and survivorship tie-breakers.*

#### 60. Standardization

*DataGroomr: Format blocks in transform rules · Module: Cleanse · SF dependency: None · First shipped: Jun 2021 · Plan: All plans*

- **What DataGroomr does:** Phone to E.164, international, national or RFC3966 (country inferred); country full/ISO-2/ISO-3; state full or 2-letter; postal codes by country; URL cleanup; SSN, EIN, card formats.
- **Generic capability:** libphonenumber, ISO country/state codes, postal formats as canonical match keys.
- **Why this rank:** Standardized keys raise match recall; mainly an input to matching.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002227715-transform-rules>

#### 61. Transform rule engine

*DataGroomr: Transform Rules (Rule Designer) · Module: Cleanse · SF dependency: None · First shipped: Sep 2021; regex Aug 2024 · Plan: All plans*

- **What DataGroomr does:** Block-based rules: case, find/replace with regex or dictionary, substrings, concatenation, math, dates, if/else. Used in cleansing, imports and Classic model preprocessing.
- **Generic capability:** SQL/UDF transformation library usable before matching and on output.
- **Why this rank:** Same engine serves cleanup and match-key prep.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002227715-transform-rules>

#### 62. Bulk transform, update & delete

*DataGroomr: Mass Transform; Mass Update; Mass Delete · Module: Cleanse · SF dependency: Partial · First shipped: Jun 2021; preview Mar 2024 · Plan: Mass transforms Professional+*

- **What DataGroomr does:** Apply transforms to all records in a score band with a before/after preview; delete junk records (deletes can be undone from the recycle bin).
- **Generic capability:** Bulk updates with preview and rollback.
- **Why this rank:** Hygiene, not dedupe.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002227712-mass-transform-mass-update->

#### 63. Record quality scoring

*DataGroomr: Data Quality models · Module: Assess · SF dependency: None · First shipped: Apr 2024; AI templates Aug 2026 · Plan: All plans*

- **What DataGroomr does:** Score each record 0-100 on completeness (percent of fields filled), format consistency, timeliness and custom rules; AI templates for incomplete, junk and stale records. Scores drive filters and mass actions.
- **Generic capability:** Quality score per record, used as a survivorship input ('most complete wins') and to triage.
- **Why this rank:** Feeds survivorship; secondary on its own.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002526884-manage-data-quality-models>

#### 64. Email verification

*DataGroomr: Verify: email · Module: Verify · SF dependency: None · First shipped: May 2021 · Plan: Credits*

- **What DataGroomr does:** Format, domain, role/shared, disposable, catch-all, SMTP mailbox check, ML misspelling detection and alternative suggestions. Score: valid 100, warning <75, risky <50, invalid <25. 1 credit (2 with a suggestion).
- **Generic capability:** Email validation via partner API; verified emails become stronger match keys.
- **Why this rank:** Commodity API. Useful as match-key confidence.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002254878-how-does-email-verification-work->

#### 65. Personal vs business email detection

*DataGroomr: Email type detection · Module: Verify · SF dependency: None · First shipped: Mar 2026 · Plan: Credits*

- **What DataGroomr does:** Flags whether an email is personal or business.
- **Generic capability:** Classify free-mail domains (gmail, yahoo) so they weigh differently in matching.
- **Why this rank:** Directly useful for person matching: a shared corporate domain is weak evidence, a personal address is strong.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002777645-march-2026>

#### 66. Phone verification

*DataGroomr: Verify: phone · Module: Verify · SF dependency: None · First shipped: May 2021; line type Sep 2024 · Plan: Credits*

- **What DataGroomr does:** Worldwide carrier lookup with line type (mobile, landline, VoIP, pager) and porting details; 2 credits flat since May 2026.
- **Generic capability:** Phone validation and line-type via partner API.
- **Why this rank:** Commodity; helps weight phone evidence.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002254913-how-does-phone-verification-work->

#### 67. Address verification

*DataGroomr: Verify: mailing address · Module: Verify · SF dependency: None · First shipped: Sep 2021 · Plan: Credits*

- **What DataGroomr does:** 250 countries; postal authority data (USPS, Royal Mail, Canada Post) plus geo sources; suggestions to replace or ignore. Needs street with number and city. 15-35 credits.
- **Generic capability:** CASS/postal validation and geocoding to canonicalize addresses before matching.
- **Why this rank:** Canonical addresses improve household and person matching.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002254934-how-does-mailing-address-verification-work->

#### 68. Email correction & discovery

*DataGroomr: Smart email suggestions; email-pattern intelligence · Module: Verify · SF dependency: None · First shipped: Jun 2021 · Plan: Credits*

- **What DataGroomr does:** Suggests a corrected or likely email from record context and learned per-domain formats; a shared library of company email patterns (Jun 2025).
- **Generic capability:** Repair typos in contact points before matching.
- **Why this rank:** Moderate value; fixes some missed matches.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002227708-smart-email-address-suggestions-email-correction->

#### 69. Suggestion review queue

*DataGroomr: Review queue (verify & enrich) · Module: Verify · SF dependency: None · First shipped: Oct 2026 · Plan: Credits*

- **What DataGroomr does:** One record at a time with current vs proposed values, per-field toggles, inline edit, Apply/Skip/Ignore with keyboard shortcuts, Apply All.
- **Generic capability:** Steward queue for any proposed attribute change.
- **Why this rank:** Good UX pattern to reuse for merge review.
- **Source:** <https://datagroomr.com/datagroomr-october26-release-notes/>

#### 70. Verification freshness & caching

*DataGroomr: Stale/changed badges; no re-charge for repeats · Module: Verify · SF dependency: None · First shipped: Aug 2025; badges Nov 2026 · Plan: Credits*

- **What DataGroomr does:** Verified values marked stale after 6 months or 'changed' when the source differs. Identical values reuse results without spending credits.
- **Generic capability:** TTL on verification results; cache by value hash.
- **Why this rank:** Cost control; minor for dedupe.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002864844-november-2026>

#### 71. Website / domain verification

*DataGroomr: Verify: website · Module: Verify · SF dependency: None · First shipped: Sep 2024 · Plan: Credits*

- **What DataGroomr does:** URL format, DNS, HTTPS certificate; detects redirects and suggests the new URL. 1 credit.
- **Generic capability:** Domain validation for company matching.
- **Why this rank:** Mostly useful for B2B company entities.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002558532-how-does-website-address-verification-work->

#### 72. Verification API

*DataGroomr: Verification API; invocable action · Module: API · SF dependency: Partial · First shipped: Aug 2024 · Plan: Credits*

- **What DataGroomr does:** Verify emails, phones and addresses from forms or external systems; Flow/OmniStudio action.
- **Generic capability:** Validation endpoint at point of entry.
- **Why this rank:** Peripheral to dedupe.
- **Source:** <https://datagroomr.com/august24-ai-assistant-machine-learning-improvements-and-more/>

#### 73. Push scores back to source

*DataGroomr: Push DQ / verification scores to Salesforce · Module: Assess · SF dependency: Partial · First shipped: Dec 2021; DQ Jun 2024 · Plan: All plans*

- **What DataGroomr does:** Writes DQ scores and verification status, score and date into source fields.
- **Generic capability:** Write quality and match metadata back to each source for reporting.
- **Why this rank:** Nice for reporting.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002527097-data-quality-model-editor>

### Tier 6: Governance & platform

*Procurement gates. Low daily value, but they block enterprise deals if missing.*

#### 74. Role-based access control

*DataGroomr: Roles (Access, Configuration, Mass Actions) · Module: Governance · SF dependency: Partial · First shipped: Sep 2021; granular Nov 2025 · Plan: Enterprise*

- **What DataGroomr does:** Inherit/Allow/Deny per module or dataset for access, configuration and mass actions (including export); restrict tags by role. Users only see what the source system lets them see.
- **Generic capability:** RBAC on datasets and actions plus row-level security passthrough from sources.
- **Why this rank:** Enterprise gate: who may mass-merge or export PII.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002256041-manage-roles>

#### 75. Security & compliance

*DataGroomr: SOC 2 Type II, HIPAA (BAA), GDPR · Module: Governance · SF dependency: None · First shipped: SOC 2 2023-24; HIPAA Jan 2024 · Plan: All plans*

- **What DataGroomr does:** TLS in transit, AES-256 at rest, OAuth tokens salted and hashed, annual pen tests. Claims to store only record IDs and metadata and fetch data live; exceptions are rollback backups, 30-day CSV staging and 30-day enrichment staging. No hosting region stated.
- **Generic capability:** Same certifications plus data-residency choice; for a DaaS, ideally run in the client's own cloud.
- **Why this rank:** Procurement gate for person data. Residency is a gap you can fill.
- **Source:** <https://datagroomr.com/security/>

#### 76. Environment promotion

*DataGroomr: Copy Configuration between orgs · Module: Platform · SF dependency: Partial · First shipped: Nov 2022; full diff Sep 2026 · Plan: Standard+*

- **What DataGroomr does:** Diff and copy 11 config types (datasets, models, rules, DQ models, enrichment actions, triggers, jobs, workflows, dictionaries) between sandbox and production.
- **Generic capability:** Config-as-code: version, diff and promote matcher configs dev → staging → prod.
- **Why this rank:** Matters to larger clients with change control.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002828014-copy-configuration-between-orgs-sandboxes-and-prod->

#### 77. Bring your own LLM / AI off switch

*DataGroomr: BYO LLM; AI Assistant toggle · Module: AI · SF dependency: None · First shipped: Oct 2025; Feb 2026 · Plan: All plans*

- **What DataGroomr does:** Default Claude on Amazon Bedrock; alternatives Anthropic, Bedrock, OpenAI, OpenRouter, Gemini, Copilot, Salesforce Einstein. Using your own model costs no credits. AI can be disabled.
- **Generic capability:** Pluggable LLM backend, including self-hosted, with a kill switch.
- **Why this rank:** Security-conscious buyers ask for it.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002687395-datagroomr-artificial-intelligence-ai->

#### 78. Service identity for automation

*DataGroomr: Job owner; auto-disable on deactivated users · Module: Governance · SF dependency: Partial · First shipped: Sep 2026 · Plan: All plans*

- **What DataGroomr does:** Jobs, triggers and workflows run as their creator; if that user is deactivated they auto-disable and admins are emailed.
- **Generic capability:** Service accounts and scoped credentials for automated runs.
- **Why this rank:** Operational detail; generic platforms solve with service accounts.
- **Source:** <https://datagroomr.com/datagroomr-september26-release-notes/>

#### 79. Agent / MCP access

*DataGroomr: MCP Servers · Module: API · SF dependency: None · First shipped: 2026 · Plan: Enterprise or trial*

- **What DataGroomr does:** Exposes tools to Claude, Agentforce and other clients with presets (read only, record actions, everything), preview-first writes, badges for irreversible or credit-spending actions, usage logs. 60 calls/min per person, 600 per server.
- **Generic capability:** MCP server over the ER API so agents can query and propose resolutions safely.
- **Why this rank:** Forward-looking; cheap to add on top of a good API.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002863255-mcp-servers>

#### 80. Accessibility

*DataGroomr: WCAG 2.1 AA · Module: UX · SF dependency: None · First shipped: Jul 2026 · Plan: All plans*

- **What DataGroomr does:** Keyboard navigation and focus management.
- **Generic capability:** Same.
- **Why this rank:** Public-sector requirement.
- **Source:** <https://datagroomr.com/datagroomr-summer26-release-notes/>

### Tier 7: Peripheral & upsell

*Revenue add-ons and polish. Little to do with dedupe quality.*

#### 81. Conversational agent

*DataGroomr: Oscar AI agent · Module: AI · SF dependency: None · First shipped: Sep 2026 · Plan: Not stated*

- **What DataGroomr does:** Chat or voice agent that diagnoses issues, builds models and rules, runs merges, verifies, enriches and builds workflows; knows the current page; asks approval before writes.
- **Generic capability:** Copilot over the ER configuration.
- **Why this rank:** Demo appeal; does not change match quality.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002848745-datagroomr-agent-your-ai-data-steward>

#### 82. Third-party enrichment orchestration

*DataGroomr: Enrich (20+ providers, custom MCP) · Module: Enrich · SF dependency: None · First shipped: May 2026 · Plan: Credits; early preview*

- **What DataGroomr does:** ZoomInfo, Apollo, D&B, Hunter, Lusha, Seamless.ai, FullEnrich and others with the client's own keys; Serper web search managed; custom MCP providers; prompt-driven execution plans; 10-100 credits per record.
- **Generic capability:** Append third-party attributes with provenance; partner rather than build.
- **Why this rank:** Since May 2026 DataGroomr's roadmap has gone mostly here. Not what dedupe buyers ask for.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002788213-enrichment-actions>

#### 83. Enrichment staging & triggers

*DataGroomr: Staged results; real-time and scheduled enrich · Module: Enrich · SF dependency: Partial · First shipped: May-Oct 2026 · Plan: Credits*

- **What DataGroomr does:** Results staged before write, manual or automatic apply, expire after 30 days; enrich on create/update or on a schedule; enrich files before load.
- **Generic capability:** Staging table with approval for appended data.
- **Why this rank:** Peripheral.
- **Source:** <https://datagroomr.com/datagroomr-october26-release-notes/>

#### 84. AI enrichment templates

*DataGroomr: Quick Start templates (ICP fit, persona, deal risk) · Module: Enrich · SF dependency: Partial · First shipped: May 2026 · Plan: Credits*

- **What DataGroomr does:** Prompt templates for company description, industry, seniority, buyer persona, outreach angle, deal risk.
- **Generic capability:** LLM-derived attributes.
- **Why this rank:** Sales-ops upsell.
- **Source:** <https://datagroomr.com/datagroomr-may26-release-notes/>

#### 85. Business-value-at-risk reporting

*DataGroomr: Revenue Exposure (Insights) · Module: Assess · SF dependency: Partial · First shipped: Aug 2026 · Plan: Not stated*

- **What DataGroomr does:** Pipeline value of duplicates tied to open opportunities.
- **Generic capability:** Value-at-risk from duplicates (open orders, contracts, tickets) for ROI stories.
- **Why this rank:** Sales enablement, not dedupe capability.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002823300-insights-report>

#### 86. Pre-send list hygiene

*DataGroomr: Campaign verification · Module: Verify · SF dependency: SF-only · First shipped: Aug 2026 · Plan: Credits*

- **What DataGroomr does:** Verify every member of a marketing campaign, optionally opt out invalid emails.
- **Generic capability:** Verify a list before a send.
- **Why this rank:** Marketing use case.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002823085-verify-emails-for-all-members-of-a-salesforce-campaign>

#### 87. UX polish

*DataGroomr: Themes, folders, keyboard shortcuts, navigation · Module: UX · SF dependency: None · First shipped: 2022-2026 · Plan: All plans*

- **What DataGroomr does:** Dark and semi-dark themes, dataset and action folders, collapsible navigation, keyboard shortcuts in queues.
- **Generic capability:** Standard polish.
- **Why this rank:** Expected, not a differentiator.
- **Source:** <https://datagroomr.com/datagroomr-september26-release-notes/>

#### 88. Credit & billing mechanics

*DataGroomr: Flex credits; auto-reload; expiry alerts · Module: Platform · SF dependency: None · First shipped: Jun 2021 · Plan: All plans*

- **What DataGroomr does:** Credits for verification, AI and enrichment; expire after 12 months; auto-reload; alerts 14 days, 7 days and 24 hours before expiry.
- **Generic capability:** Usage metering.
- **Why this rank:** Commercial plumbing.
- **Source:** <https://help.datagroomr.com/support/solutions/articles/44002231541-manage-credits>

## Salesforce artifacts removed

These DataGroomr features exist only because the product lives inside Salesforce. They are not ranked. Each maps to the generic capability that replaces it.

| DataGroomr feature | What it does in Salesforce | Generic equivalent | Maps to | Source |
|---|---|---|---|---|
| Lead → Contact / Account conversion | Single and mass convert of matched Leads; 'convert all' at confidence 0; choose status, owner, create Opportunity. Lead→Account creates a new Contact. | Promote a prospect record into an existing person entity. In a generic model this is just cross-source matching plus survivorship. | [#14](#14-cross-source-matching) | [link](https://help.datagroomr.com/support/solutions/articles/44002311261-converting-a-lead-and-mass-converting-leads) |
| Recycle-Bin-based Undo | Undo works only while losers remain in Salesforce's Recycle Bin (about 15 days). | Event-sourced, non-destructive linking makes undo unlimited. | [#12](#12-undo-a-merge) | [link](https://help.datagroomr.com/support/solutions/articles/44002265859-recycle-bin) |
| Rollback backups stored in Salesforce Files | Pre-merge snapshots saved to a 'DataGroomr' Files library, private to its creator by default. | Versioned entity store you control. | [#13](#13-full-rollback-including-field-values) | [link](https://help.datagroomr.com/support/solutions/articles/44002265857-undo-and-rollback-a-merge-) |
| Destructive native merge | Losing records are deleted by Salesforce's merge; Created By/Date come from the oldest record; triggers, flows and validation rules run in the same transaction. | Default to linking with a persistent entity ID; offer physical merge only as an optional write-back. | [#10](#10-re-point-related-records-to-the-survivor) | [link](https://help.datagroomr.com/support/solutions/articles/44002262312-what-happens-to-created-date-and-last-modified-date-when-records-are-merged-) |
| Export to Duplicate Record Sets & approve via SF reports | Writes groups into native duplicate objects; approval loop built with a custom report and picklist. | Write entity IDs back to each source; approval states on clusters. | [#42](#42-write-cluster-ids-back-to-sources) | [link](https://help.datagroomr.com/support/solutions/articles/44002621590-can-i-build-report-of-duplicates-in-salesforce-) |
| Duplicates & Verify Lightning components | Record-page widgets; Lightning-only merge seats at $100/user/yr; mobile-certified. | Embeddable widget for any host app. | [#59](#59-embeddable-duplicate-widget) | [link](https://help.datagroomr.com/support/solutions/articles/44002413768-using-duplicates-lightning-component) |
| Salesforce Flow integration | Trigger Flows on new duplicates; templates (Duplicate Detected, Verify Contact); override merge with a Flow; Duplicate Check Flow action; invocable Apex for Verify. | Webhooks plus pluggable write-back handlers. | [#44](#44-pluggable-write-back-merge-handler) | [link](https://help.datagroomr.com/support/solutions/articles/44002777748-overriding-standard-merge-method-using-salesforce-flow) |
| Apex REST merge handler | Sample Apex class receiving merge requests. | Handler interface in any language. | [#44](#44-pluggable-write-back-merge-handler) | [link](https://help.datagroomr.com/support/solutions/articles/44002332525-overriding-standard-merge-method-using-apex) |
| NPSP (nonprofit) merge | Nonprofit contacts merge switch, unlocked package, re-parents households, affiliations, relationships, contact points. | Household/group entity semantics. | [#33](#33-household--group-entities) | [link](https://help.datagroomr.com/support/solutions/articles/44002423075-can-i-merge-contacts-in-nonprofit-success-pack-npsp-) |
| Person Accounts, Cases, EmailMessages specifics | Auto-created Person/Business Account datasets; prebuilt models and merger for Cases and Email Messages. | Entity-type-specific merge semantics (person vs company vs ticket). | [#10](#10-re-point-related-records-to-the-survivor) | [link](https://help.datagroomr.com/support/solutions/articles/44002197022-can-i-deduplicate-person-accounts-) |
| Contact-to-multiple-accounts & roles cleanup | Removes redundant AccountContactRelation records; unions contact roles. | Collapse duplicate edges when re-pointing foreign keys. | [#10](#10-re-point-related-records-to-the-survivor) | [link](https://help.datagroomr.com/support/solutions/articles/44002231624-merge-contacts-with-multiple-accounts-records) |
| Bypass assignment & duplicate rules | Suppresses Salesforce assignment and duplicate rules during merge, update, rollback and export; headers passed to custom handlers. | Suppress downstream side effects while writing back. | [#44](#44-pluggable-write-back-merge-handler) | [link](https://help.datagroomr.com/support/solutions/articles/44002527106-bypass-salesforce-assignment-and-merge-rules) |
| Governor-limit throttling | Mass merge slows automatically to avoid SOQL, callout and row-lock limits. | Adaptive rate limiting per destination API. | [#9](#9-bulk-resolution-of-clusters) | [link](https://datagroomr.com/april-21-release-new-merge-rules-and-ui-enhancements/) |
| SOQL filters and date literals | SOQL WHERE mode; LAST_N_DAYS and similar. | SQL predicates. | [#54](#54-segment-scoping) | [link](https://help.datagroomr.com/support/solutions/articles/44002125898-configuring-dedupe-datasets) |
| Sandbox management (add, move after refresh, copy) | One sandbox included; extras 30% of plan; 'Move Sandbox' after refresh. | Environment promotion / config-as-code. | [#76](#76-environment-promotion) | [link](https://help.datagroomr.com/support/solutions/articles/44002191703-manage-sandboxes) |
| Salesforce field history for before/after | Audit before/after values only if Field History Tracking is on. | Own the audit log; record before/after yourself. | [#50](#50-decision-audit-trail) | [link](https://help.datagroomr.com/support/solutions/articles/44001926969-log-browser) |
| Formula / roll-up fields and two-pass 'stamp' pattern | Child values only reachable through formula or roll-up fields, or a Flow that stamps the master ID onto the parent. | Join child tables directly in SQL when building features. | [#26](#26-features-from-related-records) | [link](https://help.datagroomr.com/support/solutions/articles/44002848848-deduplicating-parent-records-based-on-values-from-child-records) |
| Account hierarchy exclusion | Uses Account.ParentId. | Relationship-based must-not-link. | [#32](#32-relationship-exclusions) | [link](https://datagroomr.com/datagroomr-december25-release-notes/) |
| Salesforce OAuth / SSO / Connected App / 2GP packaging | Salesforce is the only identity provider; Connected App install and package upgrades required. | SSO plus per-source credentials. | [#75](#75-security--compliance) | [link](https://help.datagroomr.com/support/solutions/articles/44002685489-preparing-for-salesforce-connected-app-usage-restrictions) |
| Shield encrypted fields | Matches on deterministic and probabilistic Shield-encrypted fields. | Matching on tokenized or deterministically encrypted PII (privacy-preserving ER). | [#75](#75-security--compliance) | [link](https://help.datagroomr.com/support/solutions/articles/44002350947-does-datagroomr-work-with-encrypted-fields-) |
| External Objects as the only non-SF source | Data outside Salesforce only via Salesforce Connect external objects. | Native connectors to any database, warehouse or file store. | [#14](#14-cross-source-matching) | [link](https://help.datagroomr.com/support/solutions/articles/44002364649-can-datagroomr-deduplicate-import-sync-data-outside-of-salesforce-) |
| Campaign assignment on import | Add imported or matched Leads/Contacts to Campaigns. | Drop (marketing-tool feature). | — | [link](https://datagroomr.com/datagroomr-octber25-release-notes/) |
| Einstein / Agentforce BYOM and MCP | Use Salesforce's LLM; connect Agentforce to the MCP server. | Pluggable LLM and agent access. | [#77](#77-bring-your-own-llm--ai-off-switch) | [link](https://help.datagroomr.com/support/solutions/articles/44002688556-what-s-required-to-use-salesforce-llm-) |
| Pricing by total Salesforce record count | Price computed from records across all loaded objects; removing objects lowers the bill. | Price per run, per resolved entity, or per compute. | — | [link](https://help.datagroomr.com/support/solutions/articles/44002124716-how-do-i-subscribe-to-datagroomr-) |

## What a dedupe buyer needs that DataGroomr lacks

Capabilities that entity-resolution leaders (Senzing, Splink, Zingg, AWS Entity Resolution) offer and DataGroomr does not. These are where a warehouse-native ER service can outrank it.

| Gap | Why it matters | Where it would rank |
|---|---|---|
| **Persistent entity IDs (non-destructive linking)** | DataGroomr's end state is a deleted record. A unified-profile product needs a stable entity_id per person across runs and sources, with the golden record as a view. | Insert at #2-3 |
| **Identity graph across many systems** | Cross-object matching is limited to two Salesforce objects at a time with the parent always winning. Clients with CRM + billing + support + product data need N-way resolution with lineage. | Part of #14 |
| **Term-frequency adjustment** | Common surnames and shared domains (gmail.com, a big employer) should count less. Not documented in DataGroomr; reviewers complain about over-aggressive contact matching. | Insert near #17 |
| **Evaluation tooling (precision / recall on labeled pairs)** | DataGroomr explains precision/recall in its blog but shows only a Coverage metric in product. Clients and auditors want measured accuracy per release. | Insert near #19 |
| **Configurable blocking with pair-count estimates** | Analysis has a hard 3-hour limit and the advice is to split datasets. Exposing blocking rules and estimated comparisons before a run is how you scale and price predictably. | Part of #3 |
| **Cluster-level operations and stability** | Splitting or merging whole entities, and keeping entity IDs stable when a model changes, is missing; DataGroomr pins old models to avoid re-clustering instead. | Part of #2 / #34 |
| **Data residency / run in client's cloud** | No hosting region is stated; staging stores data for 30 days. A Kubernetes-deployable engine can run inside the client's VPC. | Raise #75 |
| **Non-Salesforce sources** | G2's lowest DataGroomr sub-score is breadth of data sources (7.8). Postgres, Snowflake, BigQuery, S3/Parquet, HubSpot connectors open the rest of the market. | Foundational |
| **Usage-based pricing** | Record-count tiers make clients watch limits to avoid tier jumps. Per-run or per-entity pricing on cheap compute is a direct answer. | Commercial |

## How this was built

### Ranking method

- Lens: a client whose job-to-be-done is "dedupe my records into one entity per person", on any data source.
- Each DataGroomr capability was rewritten as a platform-agnostic capability. Salesforce-only mechanics were pulled out and mapped to their generic replacement.
- Rank weighs four signals: whether dedupe works without it, its effect on match precision or recall, its effect on the client's trust to automate, and what reviewers praise or complain about.
- Within a tier, order reflects that judgment; across tiers, a lower tier never beats a higher one.

### Caveats

- Pricing conflicts across DataGroomr's own pages (Standard $1,195 or $1,995; Professional $2,395, $2,695 or $3,995; Enterprise by quote). Treat gating as indicative; the pricing grid's tier checkmarks are images and could not be read.
- Only 19 independent reviews exist, all 4 to 5 stars, six from one January 2022 window. Customer signal is directionally useful, not statistically strong.
- The ML internals are not published. The log-odds and sigmoid display, plus early blog posts that mirror the open-source dedupe library's pipeline, suggest a logistic pairwise model with learned blocking. That is an inference.
- "First shipped" comes from release notes. 2019-2020 dates are approximate where notes are terse. November 2026 items were published ahead of release.
- "No customer data stored" is DataGroomr's claim; rollback backups, 30-day CSV staging and 30-day enrichment staging are exceptions.

## Sources

datagroomr.com release notes (May 2020 – Oct 2026), help.datagroomr.com monthly changelogs (2019 – Nov 2026) and documentation, datagroomr.com product, pricing, security and Verify pages, the DataGroomr blog, AppExchange, G2, Capterra, GetApp, Software Advice and third-party roundups. Every matrix row links to its primary source in [Capability detail](#capability-detail).
