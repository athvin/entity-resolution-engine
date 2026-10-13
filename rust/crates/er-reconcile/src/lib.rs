//! The reconcile / entity-resolution stage (port of `src/er/entities/*` +
//! `src/er/review/*`, DesignDoc.md S4.5). Pure cores first — label propagation
//! (clustering), the INV-PERM reconcile fixpoint, the never-cut fixpoint — each
//! gated against the Python; the session/matching wiring and the end-to-end
//! `entity_membership` gate land once the matching stage is in place.
//!
//! - [`cluster`]: S4.5.2 min-label propagation (connected components).

pub mod cluster;
