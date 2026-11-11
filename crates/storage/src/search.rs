//! Turns arbitrary user input into a safe FTS5 MATCH expression.

const MAX_TERMS: usize = 16;
const MAX_TERM_CHARS: usize = 64;

/// Each whitespace-separated term becomes a quoted phrase prefix (`"term"*`), so
/// operators, column filters and unbalanced quotes in user input are inert. Terms
/// without any letter or digit produce no tokens in FTS5 and are dropped.
pub(crate) fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter_map(|raw| {
            let cleaned: String = raw.chars().filter(|c| *c != '"' && !c.is_control()).take(MAX_TERM_CHARS).collect();
            cleaned.chars().any(char::is_alphanumeric).then(|| format!("\"{cleaned}\"*"))
        })
        .take(MAX_TERMS)
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_each_term_as_prefix() {
        assert_eq!(fts_query("hello wor").as_deref(), Some("\"hello\"* \"wor\"*"));
    }

    #[test]
    fn strips_quotes_and_drops_symbol_only_terms() {
        assert_eq!(fts_query("\"a\"b\" * - NEAR(").as_deref(), Some("\"ab\"* \"NEAR(\"*"));
        assert_eq!(fts_query("\" * - ^ :"), None);
        assert_eq!(fts_query("   "), None);
    }

    #[test]
    fn caps_term_count_and_length() {
        let many = (0..40).map(|i| format!("t{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(fts_query(&many).unwrap().split(' ').count(), MAX_TERMS);
        let long = "x".repeat(500);
        assert_eq!(fts_query(&long).unwrap().len(), MAX_TERM_CHARS + 3);
    }
}
