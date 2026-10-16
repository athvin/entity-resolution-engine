//! Name casing shared by the emitters. Entity and column names are `snake_case`
//! in YAML; the Rust struct and the TypeScript type are `PascalCase`.

/// `golden_record` -> `GoldenRecord`. Treats `_` and `-` as word breaks.
#[must_use]
pub fn to_pascal_case(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::to_pascal_case;

    #[test]
    fn pascalizes() {
        assert_eq!(to_pascal_case("golden_record"), "GoldenRecord");
        assert_eq!(to_pascal_case("api_key"), "ApiKey");
        assert_eq!(to_pascal_case("job"), "Job");
        assert_eq!(to_pascal_case("acct_golden_record"), "AcctGoldenRecord");
    }
}
