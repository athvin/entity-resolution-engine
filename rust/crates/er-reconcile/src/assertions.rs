//! The S4.4 assertion types shared by the never-cut fixpoint and the
//! contradiction check (port of `src/er/review/assertions.py`'s data shape).

/// `always` forces a merge; `never` forbids a co-cluster (S4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertionKind {
    Always,
    Never,
}

/// One steward assertion over a canonical record pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub assertion_id: String,
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub kind: AssertionKind,
    pub active: bool,
}
