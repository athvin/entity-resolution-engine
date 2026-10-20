//! Compile a tenant's declared `comparisons:` / `blocking:` into the settings
//! document the engine loads.
//!
//! # Why this exists
//!
//! The config DSL was already there. A tenant config declares, and has always
//! declared, exactly this:
//!
//! ```yaml
//! comparisons:
//!   given_name: { levels: [exact, "jaro_winkler:0.90", variant_match], tf: true }
//!   email:      { levels: [exact, username_exact], tf: true }
//! blocking:
//!   - { key_type: email_exact, expr: email }
//! ```
//!
//! It is validated at publish, canonicalized into `config_hash`, rendered in
//! the config studio — and **never read by anything that runs**. The level
//! tokens were free strings with no interpreter: the SQL the engine actually
//! executed came from a model JSON file named by `ER_MODEL_PATH`, authored by
//! hand out of band. So every "configurable matching" capability was
//! configurable only by editing a file on a disk the tenant cannot reach.
//!
//! This module is that missing interpreter. It is deliberately *not* a new
//! language: the token set below is read off the committed `model_test_v1.json`,
//! so compiling the committed `test.yaml` reproduces that model's structure
//! level-for-level (`tests/compile.rs` asserts it). What the compiler adds on
//! top is the parts of the DSL that had nowhere to live — per-field weights, an
//! explicit null policy, and derived keys.
//!
//! # Classic models are Fellegi-Sunter models
//!
//! A competitor would ship two engines here: a "classic" weighted-sum matcher
//! and a trained probabilistic one. We ship one. A classic model is a
//! Fellegi-Sunter model whose parameters were *declared* rather than learned:
//! [`weights_to_probabilities`] turns each field's weight into the m/u pair that
//! makes that field contribute the intended share of the score. The consequence
//! worth having is that a hand-weighted model still produces the same evidence
//! waterfall, the same per-pair explanation and the same threshold semantics as
//! a trained one — there is no second code path to keep honest.
//!
//! A compiled model is a starting point, not a trained one: [`compile`] records
//! that in the model's own provenance, and training (when it lands) replaces the
//! declared parameters with estimated ones under a new version.

use std::collections::BTreeMap;

use dedupe_core::settings::json::{
    BlockingRuleJson, ComparisonJson, ComparisonLevelJson, LinkType, SettingsJson,
};
use er_config::{ComparisonSpec, Config, MatchConstraints, NullPolicy};

/// Why a config could not be compiled. No `#[from]` sources to interpolate —
/// every variant is self-describing, because the message is shown to whoever
/// wrote the config.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("comparisons.{column} declares no levels; a comparison needs at least one")]
    NoLevels { column: String },
    #[error(
        "comparisons.{column} declares level {level:?}, which is not a comparator this engine \
         knows. Known: exact, jaro_winkler:<0..1>, levenshtein:<n>, variant_match, \
         username_exact, domain_exact, first_n:<n>, dob_same_year_month"
    )]
    UnknownLevel { column: String, level: String },
    #[error("comparisons.{column} level {level:?} has a bad parameter: {reason}")]
    BadLevelParameter {
        column: String,
        level: String,
        reason: String,
    },
    #[error(
        "the declared comparison weights total {total}, not 100. Either make them total 100 or \
         leave every weight unset and they will be shared equally"
    )]
    WeightsNotHundred { total: String },
    #[error("comparisons.{column} has weight {weight}, which is not in 0..=100")]
    WeightOutOfRange { column: String, weight: String },
}

/// How much evidence a field at full agreement contributes, in bits of log2
/// Bayes factor, when it carries 100% of the weight.
///
/// Calibrated, not guessed. Summing the committed trained model's own exact-level
/// Bayes factors across its six comparisons comes to roughly 60 bits, so that is
/// how loud a model over this corpus has to be — and a grid search over
/// base_10 confirms it: at 20 bits the compiled model finds 18 clusters where
/// there are 10 (it never accumulates enough evidence for the pairs that
/// disagree on a field or two), and at 120 bits it finds 9 — it has started
/// *merging distinct people*. 55–60 bits reproduces the committed membership
/// exactly across every disagreement ratio tried, so 60 sits inside the stable
/// region rather than on its edge.
///
/// The lesson worth keeping: louder is not safer. Over-weighting trades missed
/// duplicates for false merges, which is the more expensive mistake.
const DEFAULT_FULL_AGREEMENT_BITS: f64 = 60.0;

/// What disagreement costs, as a fraction of what agreement earns.
///
/// Well below 1.0, because a mismatched field is weaker evidence against a pair
/// than a matching field is for it: people change surnames, mistype emails and
/// move house, while coincidental exact agreement on a strong field is rare.
/// The committed trained model shows the same asymmetry — its exact-email level
/// is worth about +12.7 bits while its "all other comparisons" level costs only
/// about -2.5.
///
/// 0.2 is interior to the calibrated region: at 60 bits the committed membership
/// reproduces across 0.05–0.30, and the ratio only starts to matter once the
/// scale is pushed past 60.
const DEFAULT_DISAGREEMENT_RATIO: f64 = 0.2;

/// The scale of the weight→evidence mapping.
///
/// Weights say how fields rank *against each other*; these two say how loud the
/// whole model is. They are options rather than constants because the right
/// scale depends on how dirty the corpus is — on a corpus where duplicates
/// differ in several fields, agreement has to be worth more than disagreement
/// costs, or no cluster ever forms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompileOptions {
    /// Bits of log2 Bayes factor a field at full agreement contributes when it
    /// carries 100% of the weight.
    pub full_agreement_bits: f64,
    /// What disagreement costs, as a fraction of what agreement earns.
    pub disagreement_ratio: f64,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            full_agreement_bits: DEFAULT_FULL_AGREEMENT_BITS,
            disagreement_ratio: DEFAULT_DISAGREEMENT_RATIO,
        }
    }
}

/// The anchored side of each m/u pair: how often the *likely* outcome happens.
///
/// 0.9, not 1.0, because neither side of a comparison is ever certain — a true
/// match usually agrees on a weighted field, and a non-match usually does not.
const BASE_P: f64 = 0.9;

/// SQL words that are never column references.
const SQL_WORDS: [&str; 14] = [
    "and", "or", "not", "null", "case", "when", "then", "else", "end", "true", "false", "is",
    "like", "between",
];

/// Prefix every column reference in a blocking key expression with `side.`.
///
/// The config declares a blocking **key** — the value to block on, e.g.
/// `substr(family_name,1,4) || '|' || addr_postal`. The engine wants a join
/// **predicate**. Turning one into the other means qualifying the columns on
/// each side, which is what this does:
///
/// ```text
/// email                                  ->  l.email
/// substr(family_name,1,4) || '|' || zip  ->  substr(l.family_name,1,4) || '|' || l.zip
/// ```
///
/// An identifier is treated as a column unless it is immediately followed by
/// `(` (a function call) or is a SQL word. Single-quoted literals are skipped
/// wholesale, so a `'|'` separator — or any literal containing letters — is left
/// alone. That is the rule the committed model's hand-written blocking rules
/// follow, and `blocking_keys_become_join_predicates` pins it against them.
fn qualify(expr: &str, side: char) -> String {
    let bytes = expr.as_bytes();
    let mut out = String::with_capacity(expr.len() * 2);
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        // A string literal: copy through to the closing quote, doubled quotes
        // included, without looking for identifiers inside it.
        if c == '\'' {
            out.push(c);
            i += 1;
            while i < bytes.len() {
                out.push(bytes[i] as char);
                if bytes[i] == b'\'' {
                    // A doubled '' is an escaped quote, not the end.
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // A double-quoted identifier: already explicit, qualify it as one token.
        if c == '"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            out.push(side);
            out.push('.');
            out.push_str(&expr[start..i]);
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let word = &expr[start..i];
            // Already qualified (`l.foo`): neither half gets a prefix — the
            // alias because a `.` follows it, the column because a `.` precedes
            // it. Missing the alias half yields `l.l.foo`, which is how the
            // first version of this got it wrong.
            let follows_dot = start > 0 && bytes[start - 1] == b'.';
            let precedes_dot = expr[i..].starts_with('.');
            let already_qualified = follows_dot || precedes_dot;
            // A function call: the next non-space character is `(`.
            let is_call = expr[i..].trim_start().starts_with('(');
            if already_qualified
                || is_call
                || SQL_WORDS.contains(&word.to_ascii_lowercase().as_str())
            {
                out.push_str(word);
            } else {
                out.push(side);
                out.push('.');
                out.push_str(word);
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The conjuncts a constraints block adds to every blocking rule.
///
/// Returned as a list so the caller can `AND` them onto each rule and so each
/// one is individually testable. Empty when nothing is declared, which is what
/// keeps an existing config's compiled model unchanged.
///
/// Why blocking and not scoring: a constraint has to hold *before* similarity
/// is considered, and the only place that is literally true is candidate
/// generation. Enforcing it later would mean scoring pairs we have already
/// decided cannot match — slower, and a score that has to be overridden
/// afterwards tends to leak into something that reads it.
#[must_use]
pub fn constraint_conjuncts(constraints: &MatchConstraints) -> Vec<String> {
    let mut out = Vec::new();
    for column in &constraints.required_match {
        // Equal AND present on both sides. The NOT NULL half matters: without
        // it, two records that both lack a tax id would "agree" on it.
        out.push(format!(
            "(l.{column} = r.{column} AND l.{column} IS NOT NULL AND r.{column} IS NOT NULL)"
        ));
    }
    for column in &constraints.veto {
        // Tolerate missing, refuse conflicting. Written as NOT(conflict) so a
        // NULL on either side leaves the pair eligible.
        out.push(format!(
            "NOT (l.{column} IS NOT NULL AND r.{column} IS NOT NULL AND l.{column} <> r.{column})"
        ));
    }
    for column in &constraints.exclude_when_equal {
        // Related, not duplicate. A NULL key excludes nothing — records with no
        // household are not all in the same household.
        out.push(format!(
            "NOT (l.{column} = r.{column} AND l.{column} IS NOT NULL)"
        ));
    }
    out
}

/// Turn a declared blocking key into the join predicate the engine blocks on.
#[must_use]
pub fn blocking_predicate(expr: &str) -> String {
    format!("{} = {}", qualify(expr, 'l'), qualify(expr, 'r'))
}

/// One level's resolved SQL and label.
#[derive(Debug)]
struct Level {
    sql: String,
    label: String,
}

/// Quote a comparison column for the `_l`/`_r` sides the engine generates.
fn side(column: &str, suffix: char) -> String {
    format!("\"{column}_{suffix}\"")
}

/// Parse the numeric argument of a parameterised token (`jaro_winkler:0.9`).
fn parameter(column: &str, level: &str) -> Result<String, CompileError> {
    level
        .split_once(':')
        .map(|(_, arg)| arg.trim().to_string())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| CompileError::BadLevelParameter {
            column: column.to_string(),
            level: level.to_string(),
            reason: "expected a value after ':'".to_string(),
        })
}

/// Resolve one declared level token to the SQL the engine runs.
///
/// Every token here is either read off the committed model (so compiling
/// reproduces it) or a derived-key comparator whose SQL is the obvious DuckDB
/// spelling. Nothing is invented silently: an unrecognised token is an error
/// rather than a level that quietly never fires, which is the failure mode the
/// free-string DSL had.
fn resolve_level(column: &str, token: &str) -> Result<Level, CompileError> {
    let (l, r) = (side(column, 'l'), side(column, 'r'));
    let unknown = || CompileError::UnknownLevel {
        column: column.to_string(),
        level: token.to_string(),
    };
    match token {
        "exact" => Ok(Level {
            sql: format!("{l} = {r}"),
            label: format!("Exact match on {column}"),
        }),
        // The person lexicon's variant array: nicknames agree when the variant
        // sets intersect. Only meaningful for name columns, which is why it is
        // spelled as its own token rather than derived from the column.
        "variant_match" => Ok(Level {
            sql: "array_length(list_intersect(\"name_variants_l\", \"name_variants_r\")) >= 1"
                .to_string(),
            label: "Array intersection size >= 1".to_string(),
        }),
        // Email local part: catches one person with two domains.
        "username_exact" => Ok(Level {
            sql: format!("split_part({column}_l,'@',1) = split_part({column}_r,'@',1)"),
            label: format!("split_part({column}_l,'@',1) = split_part({column}_r,'@',1)"),
        }),
        // The other half: same employer, different person — weak evidence, which
        // is why it is a level a tenant opts into with a small weight rather than
        // something the engine assumes.
        "domain_exact" => Ok(Level {
            sql: format!("split_part({column}_l,'@',2) = split_part({column}_r,'@',2)"),
            label: format!("split_part({column}_l,'@',2) = split_part({column}_r,'@',2)"),
        }),
        "dob_same_year_month" => Ok(Level {
            sql: format!("date_trunc('month', {column}_l) = date_trunc('month', {column}_r)"),
            label: format!("date_trunc('month', {column}_l) = date_trunc('month', {column}_r)"),
        }),
        _ if token.starts_with("jaro_winkler:") => {
            let arg = parameter(column, token)?;
            let threshold: f64 = arg.parse().map_err(|_| CompileError::BadLevelParameter {
                column: column.to_string(),
                level: token.to_string(),
                reason: format!("{arg:?} is not a number"),
            })?;
            if !(0.0..=1.0).contains(&threshold) {
                return Err(CompileError::BadLevelParameter {
                    column: column.to_string(),
                    level: token.to_string(),
                    reason: "a Jaro-Winkler threshold is between 0 and 1".to_string(),
                });
            }
            // Rendered the way the committed model renders it (`>= 0.9`, not
            // `>= 0.90`), so compiling reproduces that document's SQL exactly.
            let rendered = format!("{threshold}");
            Ok(Level {
                sql: format!("jaro_winkler_similarity({l}, {r}) >= {rendered}"),
                label: format!("Jaro-Winkler distance of {column} >= {rendered}"),
            })
        }
        _ if token.starts_with("levenshtein:") => {
            let arg = parameter(column, token)?;
            let max: u32 = arg.parse().map_err(|_| CompileError::BadLevelParameter {
                column: column.to_string(),
                level: token.to_string(),
                reason: format!("{arg:?} is not a whole number of edits"),
            })?;
            Ok(Level {
                sql: format!("levenshtein({l}, {r}) <= {max}"),
                label: format!("Levenshtein distance of {column} <= {max}"),
            })
        }
        // A derived match key: compare only the first n characters. The matrix
        // calls this "first N characters"; it is how loose name spellings and
        // phone numbers with extensions are made comparable.
        _ if token.starts_with("first_n:") => {
            let arg = parameter(column, token)?;
            let n: u32 = arg.parse().map_err(|_| CompileError::BadLevelParameter {
                column: column.to_string(),
                level: token.to_string(),
                reason: format!("{arg:?} is not a whole number of characters"),
            })?;
            if n == 0 {
                return Err(CompileError::BadLevelParameter {
                    column: column.to_string(),
                    level: token.to_string(),
                    reason: "comparing the first 0 characters would match everything".to_string(),
                });
            }
            Ok(Level {
                sql: format!("substr({l}, 1, {n}) = substr({r}, 1, {n})"),
                label: format!("First {n} characters of {column} agree"),
            })
        }
        _ => Err(unknown()),
    }
}

/// The m/u pair for a level that should contribute `bits` of log2 Bayes factor.
///
/// Anchors `BASE_P` (0.9) on whichever side of the ratio keeps *both* numbers inside
/// (0, 1), then derives the other from the target bits:
///
/// * evidence FOR a match (`bits >= 0`): `m = 0.9`, `u = m / 2^bits` — a true
///   match usually lands here and a random pair rarely does;
/// * evidence AGAINST (`bits < 0`): `u = 0.9`, `m = u * 2^bits` — the
///   mirror image.
///
/// Anchoring one fixed side instead would break the thing this mapping exists
/// for. Fixing `u` and deriving `m` upward hits the probability ceiling: at 20
/// and at 10 bits `m` clamps to the same value, so an exact level and a fuzzy
/// one come out *equally* weighted, which is precisely the ordering a weighted
/// model is supposed to express. (That is not hypothetical — the first version
/// did exactly this and `a_looser_level_is_worth_less_than_a_tighter_one`
/// caught it.)
///
/// Exposed because it is the whole of the weight→parameter mapping and deserves
/// to be tested directly.
#[must_use]
pub fn weights_to_probabilities(bits: f64) -> (f64, f64) {
    // `clamp` guards the extremes: 2^bits overflows to inf (or underflows to 0)
    // well outside any weight a config can declare, and a 0 probability would
    // make the log-odds infinite.
    const FLOOR: f64 = 1e-300;
    const CEIL: f64 = 0.999_999_999;
    if bits >= 0.0 {
        let u = (BASE_P / bits.exp2()).clamp(FLOOR, CEIL);
        (BASE_P, u)
    } else {
        let m = (BASE_P * bits.exp2()).clamp(FLOOR, CEIL);
        (m, BASE_P)
    }
}

/// Build the comparison for one declared column.
fn compile_comparison(
    column: &str,
    spec: &ComparisonSpec,
    weight: f64,
    options: &CompileOptions,
) -> Result<ComparisonJson, CompileError> {
    let tokens: Vec<&str> = spec.levels.iter().filter_map(|l| l.as_deref()).collect();
    if tokens.is_empty() {
        return Err(CompileError::NoLevels {
            column: column.to_string(),
        });
    }

    let mut levels: Vec<ComparisonLevelJson> = Vec::new();

    // The null level comes first, and what it means is the tenant's choice.
    //
    // `Never` is our default and the engine's existing behaviour: either side
    // missing means this field offers no evidence, so the pair is scored on the
    // other fields. DataGroomr defaults to treating two blanks as a match, which
    // its own users hit as a false-positive trap — a thousand records with no
    // phone number are not a thousand copies of one person.
    let (l, r) = (side(column, 'l'), side(column, 'r'));
    match spec.null_policy {
        NullPolicy::Never => levels.push(ComparisonLevelJson {
            sql_condition: format!("{l} IS NULL OR {r} IS NULL"),
            label_for_charts: Some(format!("{column} is NULL")),
            is_null_level: true,
            ..blank_level()
        }),
        NullPolicy::BothBlankMatches => {
            // A positive level, not a null level: two blanks are treated as
            // agreement, and so must carry a weight and be visible in the
            // waterfall as the evidence it is.
            levels.push(ComparisonLevelJson {
                sql_condition: format!("{l} IS NULL AND {r} IS NULL"),
                label_for_charts: Some(format!("{column} blank on both sides")),
                ..weighted_level(weight / f64::from(u32::try_from(tokens.len() + 1).unwrap_or(1)))
            });
            // One side blank still offers no evidence.
            levels.push(ComparisonLevelJson {
                sql_condition: format!("{l} IS NULL OR {r} IS NULL"),
                label_for_charts: Some(format!("{column} is NULL")),
                is_null_level: true,
                ..blank_level()
            });
        }
        NullPolicy::EitherBlankMatches => levels.push(ComparisonLevelJson {
            sql_condition: format!("{l} IS NULL OR {r} IS NULL"),
            label_for_charts: Some(format!("{column} blank on either side")),
            ..weighted_level(weight / f64::from(u32::try_from(tokens.len() + 1).unwrap_or(1)))
        }),
    }

    // Declared levels, most specific first — the order they are written in.
    // Level i of n earns the share (n - i) / n of the field's agreement bits, so
    // a looser comparator is worth strictly less than a tighter one without the
    // tenant having to say so.
    let n = tokens.len() as f64;
    for (i, token) in tokens.iter().enumerate() {
        let resolved = resolve_level(column, token)?;
        let share = (n - i as f64) / n;
        let bits = options.full_agreement_bits * (weight / 100.0) * share;
        let tf_column = if spec.tf && i == 0 {
            // TF adjustment belongs on the exact level: it is what makes a
            // common surname weaker evidence than a rare one. Only the most
            // specific level, matching the committed model.
            Some(column.to_string())
        } else {
            None
        };
        levels.push(ComparisonLevelJson {
            sql_condition: resolved.sql,
            label_for_charts: Some(resolved.label),
            tf_adjustment_column: tf_column,
            ..weighted_level(bits)
        });
    }

    // And the catch-all. Disagreement is evidence against the pair.
    let against = -options.full_agreement_bits * (weight / 100.0) * options.disagreement_ratio;
    levels.push(ComparisonLevelJson {
        sql_condition: "ELSE".to_string(),
        label_for_charts: Some("All other comparisons".to_string()),
        ..weighted_level(against)
    });

    Ok(ComparisonJson {
        output_column_name: column.to_string(),
        comparison_levels: levels,
        comparison_description: Some(format!("{column} (compiled from the tenant config)")),
        extra: serde_json::Map::new(),
    })
}

/// A level with no learned parameters — the null level, which carries none.
fn blank_level() -> ComparisonLevelJson {
    ComparisonLevelJson {
        sql_condition: String::new(),
        label_for_charts: None,
        m_probability: None,
        u_probability: None,
        // Declared, not estimated: pin them so a later training pass has to
        // mint a new version rather than silently moving this model's numbers.
        fix_m_probability: true,
        fix_u_probability: true,
        is_null_level: false,
        tf_adjustment_column: None,
        tf_adjustment_weight: None,
        tf_minimum_u_value: None,
        disable_tf_exact_match_detection: false,
        extra: serde_json::Map::new(),
    }
}

/// A level whose m/u are derived from a target log2 Bayes factor.
fn weighted_level(bits: f64) -> ComparisonLevelJson {
    let (m, u) = weights_to_probabilities(bits);
    ComparisonLevelJson {
        m_probability: Some(m),
        u_probability: Some(u),
        ..blank_level()
    }
}

/// Resolve each column's weight: the declared ones if any are declared,
/// otherwise an equal share.
fn resolve_weights(
    comparisons: &BTreeMap<String, ComparisonSpec>,
) -> Result<BTreeMap<String, f64>, CompileError> {
    let declared: Vec<(&String, f64)> = comparisons
        .iter()
        .filter_map(|(c, s)| s.weight.map(|w| (c, w)))
        .collect();
    if declared.is_empty() {
        // DataGroomr's default too: 100/N each.
        let share = 100.0 / comparisons.len() as f64;
        return Ok(comparisons.keys().map(|c| (c.clone(), share)).collect());
    }
    for (column, weight) in &declared {
        if !(0.0..=100.0).contains(weight) {
            return Err(CompileError::WeightOutOfRange {
                column: (*column).clone(),
                weight: format!("{weight}"),
            });
        }
    }
    // Every column must be weighted once any is: a silent 0 for the ones left
    // out would quietly drop fields from the model.
    let total: f64 = declared.iter().map(|(_, w)| *w).sum();
    if declared.len() != comparisons.len() || (total - 100.0).abs() > 0.001 {
        return Err(CompileError::WeightsNotHundred {
            total: format!("{total}"),
        });
    }
    Ok(declared.into_iter().map(|(c, w)| (c.clone(), w)).collect())
}

/// A compiled model, with the notes a reviewer needs to judge it.
#[derive(Debug)]
pub struct Compiled {
    /// The settings document, ready to store as a model version.
    pub model_json: String,
    /// What the compiler decided that the config did not say outright.
    pub notes: Vec<String>,
}

/// Compile a tenant config into the settings document the engine loads.
///
/// # Errors
/// Returns [`CompileError`] when a declared level is unknown or malformed, a
/// comparison declares no levels, or the declared weights do not total 100.
pub fn compile(config: &Config) -> Result<Compiled, CompileError> {
    compile_with(config, &CompileOptions::default())
}

/// Compile with an explicit scale. See [`CompileOptions`].
///
/// # Errors
/// As [`compile`].
pub fn compile_with(config: &Config, options: &CompileOptions) -> Result<Compiled, CompileError> {
    let weights = resolve_weights(&config.comparisons)?;
    let conjuncts = constraint_conjuncts(&config.constraints);
    let mut notes = Vec::new();
    if config.comparisons.values().all(|s| s.weight.is_none()) {
        notes.push(format!(
            "no weights declared: the {} comparisons share the score equally ({:.1}% each)",
            config.comparisons.len(),
            100.0 / config.comparisons.len() as f64
        ));
    }

    let mut comparisons = Vec::new();
    for (column, spec) in &config.comparisons {
        let weight = weights.get(column).copied().unwrap_or(0.0);
        comparisons.push(compile_comparison(column, spec, weight, options)?);
        if spec.null_policy != NullPolicy::Never {
            notes.push(format!(
                "comparisons.{column} treats blanks as evidence ({:?}); the default is to treat a \
                 missing value as no evidence at all",
                spec.null_policy
            ));
        }
    }

    let settings = SettingsJson {
        link_type: LinkType::DedupeOnly,
        probability_two_random_records_match: 0.0001,
        retain_matching_columns: true,
        retain_intermediate_calculation_columns: false,
        additional_columns_to_retain: Vec::new(),
        sql_dialect: Some("duckdb".to_string()),
        linker_uid: None,
        em_convergence: 0.0001,
        max_iterations: 25,
        match_weight_column_prefix: "mw_".to_string(),
        term_frequency_adjustment_column_prefix: "tf_".to_string(),
        comparison_vector_value_column_prefix: "gamma_".to_string(),
        unique_id_column_name: "record_key".to_string(),
        source_dataset_column_name: "source_dataset".to_string(),
        blocking_rules_to_generate_predictions: config
            .blocking
            .iter()
            .map(|b| BlockingRuleJson::Detailed {
                blocking_rule: {
                    let mut parts = vec![blocking_predicate(&b.expr)];
                    parts.extend(conjuncts.iter().cloned());
                    parts.join(" AND ")
                },
                sql_dialect: Some("duckdb".to_string()),
                // No array-valued blocking keys from the config DSL yet: a
                // declared key is a scalar expression. `arrays_to_explode` is
                // how a model blocks on a list column, which is model-JSON-only
                // until the DSL grows a token for it.
                arrays_to_explode: Vec::new(),
                extra: serde_json::Map::new(),
            })
            .collect(),
        comparisons,
        extra: serde_json::Map::new(),
    };
    let model_json = serde_json::to_string_pretty(&settings)
        .expect("a SettingsJson built here always serializes");
    if !conjuncts.is_empty() {
        notes.push(format!(
            "{} hard constraint(s) are enforced during candidate generation, so a violating pair \
             is never scored at all: {}",
            conjuncts.len(),
            conjuncts.join(" AND ")
        ));
    }
    notes.push(
        "parameters are DECLARED, not estimated: every level is pinned (fix_m/fix_u) so a later \
         training pass mints a new version instead of moving this one's numbers"
            .to_string(),
    );
    Ok(Compiled { model_json, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraints_compile_to_candidate_conjuncts() {
        // Nothing declared -> nothing added, which is what leaves an existing
        // config's compiled model untouched.
        assert_eq!(constraint_conjuncts(&MatchConstraints::default()).len(), 0);

        // required_match: equal AND present. The presence half matters — without
        // it two records that both lack a tax id would "agree" on it.
        let required = MatchConstraints {
            required_match: vec!["tax_id".to_string()],
            ..MatchConstraints::default()
        };
        assert_eq!(
            constraint_conjuncts(&required),
            vec![
                "(l.tax_id = r.tax_id AND l.tax_id IS NOT NULL AND r.tax_id IS NOT NULL)"
                    .to_string()
            ]
        );

        // veto: tolerate missing, refuse conflicting.
        let veto = MatchConstraints {
            veto: vec!["ssn".to_string()],
            ..MatchConstraints::default()
        };
        assert_eq!(
            constraint_conjuncts(&veto),
            vec!["NOT (l.ssn IS NOT NULL AND r.ssn IS NOT NULL AND l.ssn <> r.ssn)".to_string()]
        );

        // exclude_when_equal: related, not duplicate.
        let exclude = MatchConstraints {
            exclude_when_equal: vec!["household_id".to_string()],
            ..MatchConstraints::default()
        };
        assert_eq!(
            constraint_conjuncts(&exclude),
            vec![
                "NOT (l.household_id = r.household_id AND l.household_id IS NOT NULL)".to_string()
            ]
        );

        // All three at once, in declaration order.
        let all = MatchConstraints {
            required_match: vec!["tax_id".to_string()],
            veto: vec!["ssn".to_string()],
            exclude_when_equal: vec!["household_id".to_string()],
        };
        assert_eq!(constraint_conjuncts(&all).len(), 3);
    }

    #[test]
    fn a_veto_tolerates_a_missing_value_but_not_a_conflicting_one() {
        // The semantic difference between veto and required_match, stated as the
        // truth table the SQL has to produce. Evaluated here as plain logic
        // rather than through DuckDB, which the lake e2e does.
        let veto = |l: Option<&str>, r: Option<&str>| -> bool {
            // NOT (l IS NOT NULL AND r IS NOT NULL AND l <> r)
            !(l.is_some() && r.is_some() && l != r)
        };
        assert!(veto(None, None), "both missing: eligible");
        assert!(veto(Some("1"), None), "one missing: eligible");
        assert!(veto(Some("1"), Some("1")), "agreeing: eligible");
        assert!(!veto(Some("1"), Some("2")), "conflicting: vetoed");

        let required =
            |l: Option<&str>, r: Option<&str>| -> bool { l.is_some() && r.is_some() && l == r };
        assert!(!required(None, None), "both missing: NOT a candidate");
        assert!(!required(Some("1"), None), "one missing: NOT a candidate");
        assert!(required(Some("1"), Some("1")), "agreeing: candidate");
        assert!(
            !required(Some("1"), Some("2")),
            "conflicting: not a candidate"
        );
    }

    #[test]
    fn blocking_keys_become_join_predicates() {
        // The simple case: a bare column.
        assert_eq!(blocking_predicate("email"), "l.email = r.email");
        // The committed config's compound keys, which are the real test: the
        // function name must not be qualified, and the '|' literal must be left
        // exactly alone.
        assert_eq!(
            blocking_predicate("substr(family_name,1,4) || '|' || addr_postal"),
            "substr(l.family_name,1,4) || '|' || l.addr_postal = \
             substr(r.family_name,1,4) || '|' || r.addr_postal"
        );
        assert_eq!(
            blocking_predicate("birth_date || '|' || substr(given_name,1,3)"),
            "l.birth_date || '|' || substr(l.given_name,1,3) = \
             r.birth_date || '|' || substr(r.given_name,1,3)"
        );
    }

    #[test]
    fn qualifying_leaves_literals_keywords_and_calls_alone() {
        // A literal containing letters must survive untouched — this is the case
        // a naive identifier regex gets wrong.
        assert_eq!(
            qualify("email || 'at home' || city", 'l'),
            "l.email || 'at home' || l.city"
        );
        // An escaped quote inside a literal does not end it.
        assert_eq!(qualify("x || 'it''s' || y", 'l'), "l.x || 'it''s' || l.y");
        // SQL words are not columns.
        assert_eq!(
            qualify("a IS NOT NULL AND b", 'r'),
            "r.a IS NOT NULL AND r.b"
        );
        // Nested calls: only the arguments are columns.
        assert_eq!(
            qualify("upper(trim(given_name))", 'l'),
            "upper(trim(l.given_name))"
        );
        // A double-quoted identifier is qualified as one token.
        assert_eq!(qualify("\"email\"", 'l'), "l.\"email\"");
        // An already-qualified reference is left as written.
        assert_eq!(qualify("l.email", 'l'), "l.email");
    }

    #[test]
    fn bits_map_monotonically_to_the_bayes_factor() {
        // The whole point of the weight knob: more weight, more evidence.
        let (m1, u1) = weights_to_probabilities(2.0);
        let (m2, u2) = weights_to_probabilities(8.0);
        assert!((m1 / u1).log2() < (m2 / u2).log2());
        assert!((((m1 / u1).log2()) - 2.0).abs() < 1e-9);
        assert!((((m2 / u2).log2()) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn probabilities_stay_probabilities_at_the_extremes() {
        for bits in [-40.0, -1.0, 0.0, 1.0, 20.0, 200.0] {
            let (m, u) = weights_to_probabilities(bits);
            assert!(m > 0.0 && m < 1.0, "m out of range at {bits} bits: {m}");
            assert!(u > 0.0 && u < 1.0, "u out of range at {bits} bits: {u}");
        }
    }

    #[test]
    fn an_unknown_level_is_an_error_not_a_level_that_never_fires() {
        let err = resolve_level("email", "soundex").unwrap_err();
        assert!(matches!(err, CompileError::UnknownLevel { .. }));
        // And the message tells the author what they can write instead.
        assert!(err.to_string().contains("jaro_winkler"));
    }

    #[test]
    fn parameterised_levels_validate_their_argument() {
        assert!(matches!(
            resolve_level("family_name", "jaro_winkler:nope").unwrap_err(),
            CompileError::BadLevelParameter { .. }
        ));
        assert!(matches!(
            resolve_level("family_name", "jaro_winkler:1.5").unwrap_err(),
            CompileError::BadLevelParameter { .. }
        ));
        assert!(matches!(
            resolve_level("family_name", "first_n:0").unwrap_err(),
            CompileError::BadLevelParameter { .. }
        ));
        assert!(matches!(
            resolve_level("family_name", "jaro_winkler:").unwrap_err(),
            CompileError::BadLevelParameter { .. }
        ));
    }

    #[test]
    fn the_committed_models_level_sql_is_what_the_tokens_render() {
        // These exact strings are read off fixtures/static/model_test_v1.json.
        // If the renderer drifts from them, compiling the committed config stops
        // reproducing the committed model — which tests/compile.rs then fails.
        assert_eq!(
            resolve_level("given_name", "exact").unwrap().sql,
            "\"given_name_l\" = \"given_name_r\""
        );
        assert_eq!(
            resolve_level("given_name", "jaro_winkler:0.90")
                .unwrap()
                .sql,
            "jaro_winkler_similarity(\"given_name_l\", \"given_name_r\") >= 0.9"
        );
        assert_eq!(
            resolve_level("given_name", "variant_match").unwrap().sql,
            "array_length(list_intersect(\"name_variants_l\", \"name_variants_r\")) >= 1"
        );
        assert_eq!(
            resolve_level("email", "username_exact").unwrap().sql,
            "split_part(email_l,'@',1) = split_part(email_r,'@',1)"
        );
        assert_eq!(
            resolve_level("birth_date", "dob_same_year_month")
                .unwrap()
                .sql,
            "date_trunc('month', birth_date_l) = date_trunc('month', birth_date_r)"
        );
    }

    #[test]
    fn weights_must_total_a_hundred_once_any_is_declared() {
        let spec = |weight: Option<f64>| ComparisonSpec {
            levels: vec![Some("exact".to_string())],
            tf: false,
            weight,
            null_policy: NullPolicy::Never,
        };
        // None declared -> equal shares.
        let equal: BTreeMap<String, ComparisonSpec> =
            [("a".to_string(), spec(None)), ("b".to_string(), spec(None))].into();
        assert_eq!(
            resolve_weights(&equal).unwrap(),
            [("a".to_string(), 50.0), ("b".to_string(), 50.0)].into()
        );

        // Declared and totalling 100 -> taken as written.
        let declared: BTreeMap<String, ComparisonSpec> = [
            ("a".to_string(), spec(Some(70.0))),
            ("b".to_string(), spec(Some(30.0))),
        ]
        .into();
        assert_eq!(
            resolve_weights(&declared).unwrap(),
            [("a".to_string(), 70.0), ("b".to_string(), 30.0)].into()
        );

        // Declared but not totalling 100 -> refused, with the total named.
        let wrong: BTreeMap<String, ComparisonSpec> = [
            ("a".to_string(), spec(Some(70.0))),
            ("b".to_string(), spec(Some(20.0))),
        ]
        .into();
        let err = resolve_weights(&wrong).unwrap_err();
        assert!(err.to_string().contains("90"), "{err}");

        // Partially declared -> refused too: the undeclared field would
        // otherwise silently fall out of the model.
        let partial: BTreeMap<String, ComparisonSpec> = [
            ("a".to_string(), spec(Some(100.0))),
            ("b".to_string(), spec(None)),
        ]
        .into();
        assert!(resolve_weights(&partial).is_err());
    }

    #[test]
    fn the_null_policy_decides_what_a_blank_means() {
        let base = ComparisonSpec {
            levels: vec![Some("exact".to_string())],
            tf: false,
            weight: None,
            null_policy: NullPolicy::Never,
        };
        // Default: a missing value is a null level — no evidence either way.
        let c = compile_comparison("email", &base, 100.0, &CompileOptions::default()).unwrap();
        let null_levels: Vec<_> = c
            .comparison_levels
            .iter()
            .filter(|l| l.is_null_level)
            .collect();
        assert_eq!(null_levels.len(), 1);
        assert_eq!(
            null_levels[0].sql_condition,
            "\"email_l\" IS NULL OR \"email_r\" IS NULL"
        );
        assert!(
            null_levels[0].m_probability.is_none(),
            "a null level carries no weight"
        );

        // Both-blank-matches: a positive level that must carry weight, placed
        // before the one-sided null level so it wins.
        let both = ComparisonSpec {
            null_policy: NullPolicy::BothBlankMatches,
            ..base.clone()
        };
        let c = compile_comparison("email", &both, 100.0, &CompileOptions::default()).unwrap();
        assert_eq!(
            c.comparison_levels[0].sql_condition,
            "\"email_l\" IS NULL AND \"email_r\" IS NULL"
        );
        assert!(c.comparison_levels[0].m_probability.is_some());
        assert!(!c.comparison_levels[0].is_null_level);
        assert!(
            c.comparison_levels[1].is_null_level,
            "one side blank still means no evidence"
        );
    }

    #[test]
    fn a_looser_level_is_worth_less_than_a_tighter_one() {
        let spec = ComparisonSpec {
            levels: vec![
                Some("exact".to_string()),
                Some("jaro_winkler:0.9".to_string()),
            ],
            tf: false,
            weight: None,
            null_policy: NullPolicy::Never,
        };
        let c =
            compile_comparison("family_name", &spec, 100.0, &CompileOptions::default()).unwrap();
        let bits =
            |l: &ComparisonLevelJson| (l.m_probability.unwrap() / l.u_probability.unwrap()).log2();
        // [null, exact, jaro, ELSE]
        assert!(
            bits(&c.comparison_levels[1]) > bits(&c.comparison_levels[2]),
            "exact must outweigh fuzzy"
        );
        assert!(
            bits(&c.comparison_levels[3]) < 0.0,
            "disagreement must count against the pair"
        );
    }

    #[test]
    fn tf_adjustment_lands_on_the_exact_level_only() {
        let spec = ComparisonSpec {
            levels: vec![
                Some("exact".to_string()),
                Some("jaro_winkler:0.9".to_string()),
            ],
            tf: true,
            weight: None,
            null_policy: NullPolicy::Never,
        };
        let c =
            compile_comparison("family_name", &spec, 100.0, &CompileOptions::default()).unwrap();
        let tf: Vec<_> = c
            .comparison_levels
            .iter()
            .filter(|l| l.tf_adjustment_column.is_some())
            .collect();
        assert_eq!(tf.len(), 1, "exactly one level carries the TF adjustment");
        assert_eq!(tf[0].sql_condition, "\"family_name_l\" = \"family_name_r\"");
    }
}
