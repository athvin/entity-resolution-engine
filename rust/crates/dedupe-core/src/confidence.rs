//! The 1–100 confidence projection: the single owner of how a match weight or
//! a match probability becomes the number a person is shown.
//!
//! Every surface — the review inbox, the duplicates view, the widget, the
//! merge-plans export, any future export — reads this projection. A surface
//! that maps a score itself is how the review screen and the CSV disagree, so
//! the rule is that servers ship what [`Confidence::display`] returns and
//! clients render it verbatim.
//!
//! The rounding rule, stated: **display is the floor of the raw value** — a
//! confidence displays `N` exactly when its raw value lies in `[N, N+1)`.
//! Floor rather than round-half-up so that what is shown and what passes a
//! threshold can never disagree: a raw 79.6 displays `79`, and a threshold of
//! 80 excludes it. Round-half-up would display `80` for a score the same
//! threshold excludes — the failure the spec's boundary case ("threshold 80
//! excludes 79.6") names. Threshold comparisons themselves run on the raw
//! value via [`Confidence::meets`], never on the displayed integer.

use crate::scoring::match_weight_to_prob;

/// A match confidence on the 0–100 scale: one hundred times the model's match
/// probability. (The spec calls the scale "1–100"; the raw range is
/// `[0, 100]` and only scores well above 1 ever reach a human, so no
/// rescaling is applied.)
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    /// Project a match probability. Out-of-range input is clamped; a NaN has
    /// no meaning and maps to 0 rather than panicking a read path serving a
    /// page of otherwise-good rows.
    #[must_use]
    pub fn from_probability(p: f64) -> Self {
        let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
        Self(p * 100.0)
    }

    /// Project a match weight (log2 Bayes factor, prior included) through the
    /// engine's own weight→probability map, so the two routes to a confidence
    /// can never diverge.
    #[must_use]
    pub fn from_match_weight(w: f64) -> Self {
        Self::from_probability(match_weight_to_prob(w))
    }

    /// Wrap an already-projected raw value (e.g. a mean of confidences).
    #[must_use]
    fn from_raw(raw: f64) -> Self {
        let raw = if raw.is_nan() { 0.0 } else { raw.clamp(0.0, 100.0) };
        Self(raw)
    }

    /// The unrounded value, for thresholds, sorting and averaging.
    #[must_use]
    pub fn value(self) -> f64 {
        self.0
    }

    /// What a person is shown: the floor of the raw value. Displays `N` iff
    /// raw ∈ `[N, N+1)`; a raw 100 displays `100`.
    #[must_use]
    pub fn display(self) -> i64 {
        self.0.floor() as i64
    }

    /// Whether the confidence clears `threshold`, compared on the raw value —
    /// this is the comparison that makes "threshold 80 excludes 79.6" true
    /// even though no displayed digit says so.
    #[must_use]
    pub fn meets(self, threshold: f64) -> bool {
        self.0 >= threshold
    }
}

/// The confidence of one duplicate group: every member's attachment to the
/// group and their average, with the master inside it.
///
/// A member's **attachment** is its strongest within-group edge — how hard the
/// evidence pulls it into this group. Pair scores are supplied by the caller
/// as probabilities; assertion-sourced edges arrive as `1.0`, matching S4.4's
/// treatment of an assertion as a score-1.0 edge. A member with no
/// within-group edge at all (possible only through a cut path) carries no
/// score and is out of the average rather than scored 0 — a 0 would state
/// evidence of non-membership, which is the opposite of what its presence in
/// the group says.
///
/// The **master** is the strongest-attached member, ties broken by the lower
/// record key, so the answer is deterministic and free of any survivorship
/// policy: a policy-elected master (merge plans elect one per request) would
/// make the same group score differently on different surfaces, which is the
/// defect this type exists to remove. The group score is the arithmetic mean
/// of every member's attachment, the master included — the spec's formula —
/// and not the mean over the non-master members, which is the easy off-by-one.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupScore {
    /// The strongest-attached member's record key.
    pub master: String,
    /// `(record_key, attachment)` per scored member, ordered by record key.
    pub members: Vec<(String, Confidence)>,
    /// The arithmetic mean of every scored member's attachment, master included.
    pub group: Confidence,
}

/// Score one group from its member record keys and the within-group edges as
/// `(rec_a, rec_b, match_probability)` triples. `None` when no member has any
/// edge — the group carries no score at all, rather than a fabricated one.
#[must_use]
pub fn group_score(members: &[String], edges: &[(String, String, f64)]) -> Option<GroupScore> {
    use std::collections::BTreeMap;

    let mut attachment: BTreeMap<&str, f64> = BTreeMap::new();
    for (a, b, p) in edges {
        let raw = Confidence::from_probability(*p).value();
        attachment
            .entry(a.as_str())
            .and_modify(|v| *v = v.max(raw))
            .or_insert(raw);
        attachment
            .entry(b.as_str())
            .and_modify(|v| *v = v.max(raw))
            .or_insert(raw);
    }
    // Only actual members may score; an edge naming a stranger is discarded.
    let scored: Vec<(&str, f64)> = members
        .iter()
        .filter_map(|m| attachment.get(m.as_str()).map(|&v| (m.as_str(), v)))
        .collect();
    if scored.is_empty() {
        return None;
    }
    // Strict `>` over a record-key-ordered map: the first holder of the max
    // keeps it, so a tie resolves to the lower record key.
    let master = scored
        .iter()
        .fold(("", f64::NEG_INFINITY), |best, &(key, v)| {
            if v > best.1 { (key, v) } else { best }
        })
        .0;
    let mean = scored.iter().map(|&(_, v)| v).sum::<f64>() / scored.len() as f64;
    Some(GroupScore {
        master: master.to_string(),
        members: scored
            .into_iter()
            .map(|(k, v)| (k.to_string(), Confidence::from_raw(v)))
            .collect(),
        group: Confidence::from_raw(mean),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_floor() {
        assert_eq!(Confidence::from_probability(0.796).display(), 79);
        assert_eq!(Confidence::from_probability(0.799_999_9).display(), 79);
        assert_eq!(Confidence::from_probability(0.80).display(), 80);
        assert_eq!(Confidence::from_probability(0.0).display(), 0);
        assert_eq!(Confidence::from_probability(1.0).display(), 100);
    }

    #[test]
    fn threshold_80_excludes_79_6() {
        // The spec's boundary case: a threshold compares the raw value, so
        // 79.6 fails 80 even though that is invisible once displayed.
        let c = Confidence::from_probability(0.796);
        assert!(!c.meets(80.0));
        assert_eq!(c.display(), 79, "and the display must agree it is below 80");
        assert!(Confidence::from_probability(0.80).meets(80.0));
    }

    #[test]
    fn weight_and_probability_agree() {
        let w = 3.0; // log2 odds of 8:1
        let p = match_weight_to_prob(w);
        assert_eq!(
            Confidence::from_match_weight(w),
            Confidence::from_probability(p)
        );
    }

    #[test]
    fn out_of_range_and_nan_are_contained() {
        assert_eq!(Confidence::from_probability(1.5).display(), 100);
        assert_eq!(Confidence::from_probability(-0.2).display(), 0);
        assert_eq!(Confidence::from_probability(f64::NAN).display(), 0);
    }

    #[test]
    fn a_pair_scores_itself() {
        let members = vec!["a".to_string(), "b".to_string()];
        let edges = vec![("a".to_string(), "b".to_string(), 0.9)];
        let g = group_score(&members, &edges).unwrap();
        assert_eq!(g.group.value(), 90.0);
        assert_eq!(g.master, "a", "a tie resolves to the lower record key");
        assert_eq!(g.members.len(), 2);
    }

    #[test]
    fn a_chain_averages_every_member() {
        let members = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.9),
            ("b".to_string(), "c".to_string(), 0.8),
        ];
        let g = group_score(&members, &edges).unwrap();
        // attachments: a=90, b=90, c=80 → mean 86.66…, displayed 86.
        assert!((g.group.value() - 260.0 / 3.0).abs() < 1e-9);
        assert_eq!(g.group.display(), 86);
        assert_eq!(g.master, "a");
    }

    #[test]
    fn the_master_is_in_the_average() {
        // The spec's formula is the mean over ALL members including the
        // master; reject the master-excluded version by computing both.
        let members = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.9),
            ("b".to_string(), "c".to_string(), 0.8),
        ];
        let g = group_score(&members, &edges).unwrap();
        let with_master = (90.0 + 90.0 + 80.0) / 3.0;
        let without_master = (90.0 + 80.0) / 2.0;
        assert_eq!(g.group.value(), with_master);
        assert_ne!(g.group.value(), without_master);
    }

    #[test]
    fn an_assertion_edge_is_a_hundred() {
        let members = vec!["a".to_string(), "b".to_string(), "x".to_string()];
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.7),
            ("a".to_string(), "x".to_string(), 1.0), // assertion-sourced
        ];
        let g = group_score(&members, &edges).unwrap();
        // Both assertion-edge endpoints attach at 100; the tie resolves to the
        // lower record key, not to either endpoint at random.
        assert_eq!(g.master, "a");
        assert_eq!(
            g.members
                .iter()
                .find(|(k, _)| k == "x")
                .map(|(_, c)| c.display()),
            Some(100)
        );
    }

    #[test]
    fn a_member_with_no_edge_is_out_of_the_average() {
        let members = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "orphan".to_string(),
        ];
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.9),
            ("b".to_string(), "c".to_string(), 0.8),
        ];
        let g = group_score(&members, &edges).unwrap();
        assert_eq!(g.members.len(), 3, "the orphan carries no score");
        assert!((g.group.value() - 260.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn no_edges_means_no_score() {
        let members = vec!["a".to_string(), "b".to_string()];
        assert_eq!(group_score(&members, &[]), None);
    }
}
