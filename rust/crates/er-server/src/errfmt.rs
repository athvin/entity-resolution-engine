//! Flattening an error and its `.source()` chain into a single line.
//!
//! Most of er-server records failures as plain `String`s — the job row's
//! failure text, an HTTP error body — rather than propagating a typed error up
//! to a reporter. That makes the point of stringification the only place the
//! cause can still be recovered.
//!
//! Error types in this workspace deliberately keep their cause OUT of
//! `Display`: interpolating it there double-prints under any reporter that
//! walks `.source()` (`anyhow`'s `{:#}`, for one). The flip side is that
//! `e.to_string()` on its own now yields only the outermost message, so every
//! site that turns one of those errors into a string has to walk the chain
//! itself. [`chain`] is that walk.
//!
//! `anyhow::Error` does not need this — its own `{:#}` alternate formatter
//! already renders the full chain, and call sites here use that directly.

/// Render `e` and every error beneath it as `outermost: next: innermost`.
///
/// ```ignore
/// // "config is not valid YAML or does not match S6: invalid type at line 3 column 9"
/// errfmt::chain(&config_error)
/// ```
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cursor = e.source();
    while let Some(source) = cursor {
        out.push_str(": ");
        out.push_str(&source.to_string());
        cursor = source.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::chain;
    use std::error::Error;
    use std::fmt;

    // Hand-rolled rather than derived: er-server does not depend on thiserror,
    // and two test fixtures do not justify adding it.
    #[derive(Debug)]
    struct Inner;
    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("inner")
        }
    }
    impl Error for Inner {}

    #[derive(Debug)]
    struct Outer(Inner);
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer")
        }
    }
    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn flattens_the_whole_chain() {
        assert_eq!(chain(&Outer(Inner)), "outer: inner");
    }

    #[test]
    fn a_sourceless_error_is_just_its_message() {
        assert_eq!(chain(&Inner), "inner");
    }
}
