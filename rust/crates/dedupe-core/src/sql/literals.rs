//! CPython-compatible float formatting (`repr(float)`), used when baking
//! trained parameters into SQL so generated text diffs cleanly against the
//! captured golden SQL. Shortest round-trip digits; scientific notation iff the decimal
//! exponent is < -4 or >= 16; integral floats keep a trailing `.0`.

pub fn pyrepr(x: f64) -> String {
    assert!(x.is_finite(), "cannot format non-finite float: {x}");
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }

    let neg = x < 0.0;
    // Rust's LowerExp produces shortest round-trip digits: "d[.ddd]e<exp>".
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp_str) = sci.split_once('e').expect("exp format");
    let exp: i32 = exp_str.parse().expect("exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    let body = if (-4..16).contains(&exp) {
        decimal_from_digits(&digits, exp)
    } else {
        python_sci(&digits, exp)
    };
    if neg { format!("-{body}") } else { body }
}

fn decimal_from_digits(digits: &str, exp: i32) -> String {
    let n = digits.len() as i32;
    if exp >= n - 1 {
        // Integral: pad with zeros, append ".0" (Python shows it).
        let zeros = (exp - (n - 1)) as usize;
        format!("{digits}{}.0", "0".repeat(zeros))
    } else if exp >= 0 {
        let split = (exp + 1) as usize;
        format!("{}.{}", &digits[..split], &digits[split..])
    } else {
        let zeros = (-exp - 1) as usize;
        format!("0.{}{digits}", "0".repeat(zeros))
    }
}

fn python_sci(digits: &str, exp: i32) -> String {
    let mantissa = if digits.len() == 1 {
        digits.to_string()
    } else {
        format!("{}.{}", &digits[..1], &digits[1..])
    };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exp.abs())
}

#[cfg(test)]
mod tests {
    use super::pyrepr;

    #[test]
    fn matches_python_repr() {
        // Each expectation verified against CPython repr().
        assert_eq!(pyrepr(0.0), "0.0");
        assert_eq!(pyrepr(1.0), "1.0");
        assert_eq!(pyrepr(-2.0), "-2.0");
        assert_eq!(pyrepr(100.0), "100.0");
        assert_eq!(pyrepr(0.95), "0.95");
        assert_eq!(pyrepr(6.309110459952498), "6.309110459952498");
        assert_eq!(pyrepr(-8.386106589561935), "-8.386106589561935");
        assert_eq!(pyrepr(0.006187690390473553), "0.006187690390473553");
        assert_eq!(pyrepr(0.0001), "0.0001");
        assert_eq!(pyrepr(0.00001), "1e-05");
        assert_eq!(pyrepr(1e-300), "1e-300");
        assert_eq!(pyrepr(2.5e-10), "2.5e-10");
        assert_eq!(pyrepr(1e16), "1e+16");
        assert_eq!(pyrepr(1.5e20), "1.5e+20");
        assert_eq!(pyrepr(9999999999999998.0), "9999999999999998.0");
        assert_eq!(pyrepr(1e15), "1000000000000000.0");
    }

    #[test]
    fn round_trips() {
        for &v in &[
            6.309110459952498_f64,
            -7.336383273282888,
            1e-300,
            0.00298012298012298,
            123456.789,
        ] {
            assert_eq!(pyrepr(v).parse::<f64>().unwrap(), v);
        }
    }
}
