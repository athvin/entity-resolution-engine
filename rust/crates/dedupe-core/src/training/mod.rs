//! Parameter training: prior estimation, u estimation, and EM.
//!
//! # A fourth m estimator was considered and declined
//!
//! The profiling design proposed estimating `m` directly from the pairs
//! `training.deterministic_rules` selects — they are near-certain matches, so
//! their level distribution is an m estimate with no EM — and offered it as a
//! diagnostic, since `m̂/û` per level gives the implied bits per field.
//!
//! It is not built, deliberately. There are already three paths to `m` and they
//! cover the ground:
//!
//! - [`em`] estimates it, which is the method the model is defined by;
//! - [`m_from_labels`] replaces it from a tenant's own verdicts, which are
//!   better evidence than any rule;
//! - [`prior`] already consumes the deterministic rules, so a second consumer
//!   of the same rules would be a second thing to keep agreeing with them.
//!
//! The diagnostic it was wanted for arrived another way:
//! `TrainedModel::full_agreement_bits` reports the implied bits per field from
//! the trained model, and `TrainedModel::defaulted_levels` says which of those
//! came from a measurement rather than a default. That answers the question
//! without adding a fourth estimator whose disagreements with EM would need
//! explaining every time they differed.
//!
//! Worth revisiting only if EM's own `m` is shown to be wrong on a real corpus,
//! in which case the deterministic-rule distribution is the obvious check —
//! but then it is a *test*, not a production estimator.

pub mod em;
pub mod estimate_u;
pub mod m_from_labels;
pub mod prior;
