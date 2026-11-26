/// The outer parenthesized reads in the owned `RETURN [(SELECT ...), ...]`
/// batch format. Nested selects and quoted parentheses are not extra groups.
/// This controlled-adapter utility is not a general SurrealQL parser.
pub fn grouped_select_queries(sql: &str) -> Vec<&str> {
    let Some(body) = sql.strip_prefix("RETURN [") else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in body.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => {
                if depth == 0 {
                    start = index + 1;
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    groups.push(&body[start..index]);
                }
            }
            _ => {}
        }
    }
    groups
}
