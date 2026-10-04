//! Bounded Rust source lexer shared by the source/export/caller guard tests.
//!
//! This is deliberately not a Rust parser. It recognizes lexical tokens,
//! removes comments and string/character literals from the token stream, and
//! reports unterminated constructs. Callers must diagnose syntax they cannot
//! interpret instead of treating an empty parse as success.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) text: String,
}

pub(crate) fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        let current = chars[index];
        if current.is_whitespace() {
            index += 1;
            continue;
        }

        if current == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if current == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            let mut depth = 1usize;
            while index < chars.len() && depth > 0 {
                if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                    depth += 1;
                    index += 2;
                } else if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            if depth != 0 {
                return Err("unterminated block comment".into());
            }
            continue;
        }

        if let Some((content_start, hashes)) = raw_string_start(&chars, index) {
            index = content_start;
            let mut found_end = false;
            while index < chars.len() {
                if chars[index] == '"'
                    && (0..hashes).all(|offset| chars.get(index + 1 + offset) == Some(&'#'))
                {
                    index += 1 + hashes;
                    found_end = true;
                    break;
                }
                index += 1;
            }
            if !found_end {
                return Err("unterminated raw string literal".into());
            }
            tokens.push(Token {
                text: "<literal>".into(),
            });
            continue;
        }

        if current == '"'
            || (current == 'b' && chars.get(index + 1) == Some(&'"'))
            || (current == 'c' && chars.get(index + 1) == Some(&'"'))
        {
            if current != '"' {
                index += 1;
            }
            index = skip_quoted(&chars, index, '"')?;
            tokens.push(Token {
                text: "<literal>".into(),
            });
            continue;
        }

        if current == '\'' || (current == 'b' && chars.get(index + 1) == Some(&'\'')) {
            let quote = if current == 'b' { index + 1 } else { index };
            if let Some(end) = char_literal_end(&chars, quote, current == 'b') {
                index = end;
                tokens.push(Token {
                    text: "<literal>".into(),
                });
                continue;
            }
            // A lifetime (`'a`) is punctuation followed by an identifier, not
            // a character literal. Keep tokenizing it normally.
            if current == 'b' {
                tokens.push(Token { text: "b".into() });
                index += 1;
            } else {
                tokens.push(Token { text: "'".into() });
                index += 1;
            }
            continue;
        }

        if is_ident_start(current) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            tokens.push(Token {
                text: chars[start..index].iter().collect(),
            });
            continue;
        }

        tokens.push(Token {
            text: current.to_string(),
        });
        index += 1;
    }
    Ok(tokens)
}

pub(crate) fn matching_group(tokens: &[Token], open: usize) -> Option<usize> {
    let opening = tokens.get(open)?.text.as_str();
    let closing = match opening {
        "{" => "}",
        "(" => ")",
        "[" => "]",
        _ => return None,
    };
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == opening {
            depth += 1;
        } else if token.text == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

pub(crate) fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(is_ident_continue)
}

pub(crate) fn is_open_group(token: &str) -> bool {
    matches!(token, "{" | "(" | "[")
}

/// Find a direct or turbofish call, treating const-expression groups as opaque.
pub(crate) fn call_open(tokens: &[Token], after_name: usize) -> Option<usize> {
    if tokens.get(after_name)?.text == "(" {
        return Some(after_name);
    }
    if tokens.get(after_name)?.text != ":"
        || tokens.get(after_name + 1)?.text != ":"
        || tokens.get(after_name + 2)?.text != "<"
    {
        return None;
    }
    let mut depth = 1usize;
    let mut index = after_name + 3;
    while index < tokens.len() {
        let text = tokens[index].text.as_str();
        if is_open_group(text) {
            index = matching_group(tokens, index)? + 1;
            continue;
        }
        match text {
            "<" => depth += 1,
            ">" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return tokens
                        .get(index + 1)
                        .is_some_and(|token| token.text == "(")
                        .then_some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn raw_string_start(chars: &[char], index: usize) -> Option<(usize, usize)> {
    let raw_index = if chars.get(index) == Some(&'b') && chars.get(index + 1) == Some(&'r') {
        index + 1
    } else if chars.get(index) == Some(&'r') {
        index
    } else {
        return None;
    };
    let mut cursor = raw_index + 1;
    while chars.get(cursor) == Some(&'#') {
        cursor += 1;
    }
    (chars.get(cursor) == Some(&'"')).then_some((cursor + 1, cursor - raw_index - 1))
}

fn skip_quoted(chars: &[char], quote_index: usize, quote: char) -> Result<usize, String> {
    let mut cursor = quote_index + 1;
    while cursor < chars.len() {
        match chars[cursor] {
            '\\' => cursor = (cursor + 2).min(chars.len()),
            found if found == quote => return Ok(cursor + 1),
            _ => cursor += 1,
        }
    }
    Err("unterminated quoted literal".into())
}

fn char_literal_end(chars: &[char], quote_index: usize, is_byte: bool) -> Option<usize> {
    let mut cursor = quote_index + 1;
    match *chars.get(cursor)? {
        '\\' => cursor = char_escape_end(chars, cursor, is_byte)?,
        character
            if character != '\''
                && character != '\n'
                && character != '\r'
                && (!is_byte || character.is_ascii()) =>
        {
            cursor += 1;
        }
        _ => return None,
    }
    (chars.get(cursor) == Some(&'\'')).then_some(cursor + 1)
}

fn char_escape_end(chars: &[char], escape_start: usize, is_byte: bool) -> Option<usize> {
    match *chars.get(escape_start + 1)? {
        '\'' | '"' | '\\' | 'n' | 'r' | 't' | '0' => Some(escape_start + 2),
        'x' => {
            let high = chars.get(escape_start + 2)?.to_digit(16)?;
            let low = chars.get(escape_start + 3)?.to_digit(16)?;
            let value = high * 16 + low;
            (is_byte || value <= 0x7f).then_some(escape_start + 4)
        }
        'u' if !is_byte && chars.get(escape_start + 2) == Some(&'{') => {
            let mut cursor = escape_start + 3;
            let mut value = 0u32;
            let mut digits = 0;
            while let Some(character) = chars.get(cursor) {
                match character {
                    '}' => {
                        return (digits > 0 && char::from_u32(value).is_some())
                            .then_some(cursor + 1);
                    }
                    '_' => cursor += 1,
                    character if character.is_ascii_hexdigit() => {
                        digits += 1;
                        if digits > 6 {
                            return None;
                        }
                        value = value * 16 + character.to_digit(16)?;
                        cursor += 1;
                    }
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

fn is_ident_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_ident_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_generic_comparisons_do_not_end_a_call_early() {
        let tokens = tokenize("call::<{ 1 < 2 }, [u8; 3]>();").expect("call tokens");
        let opening = call_open(&tokens, 1).expect("balanced generic call");
        assert_eq!(tokens[opening].text, "(");
    }

    #[test]
    fn greater_than_inside_a_const_group_does_not_close_the_turbofish() {
        let tokens = tokenize("call::<[u8; { 3 > 2 }]>();").expect("call tokens");
        let opening = call_open(&tokens, 1).expect("balanced generic call");
        assert_eq!(tokens[opening].text, "(");
    }

    #[test]
    fn string_and_comment_text_is_not_a_code_token() {
        let tokens = tokenize(
            r##"
                // mod comment_only;
                /* outer /* nested */ comment */
                const TEXT: &str = r#"mod raw_string_only;"#;
                let character = 'm';
            "##,
        )
        .expect("tokenize");
        assert!(!tokens.iter().any(|token| token.text == "comment_only"));
        assert!(!tokens.iter().any(|token| token.text == "raw_string_only"));
        assert!(!tokens.iter().any(|token| token.text == "m"));
    }

    #[test]
    fn unclosed_comments_and_strings_fail_instead_of_looking_empty() {
        assert!(tokenize("/* never closed").is_err());
        assert!(tokenize("\"never closed").is_err());
        assert!(tokenize("r###\"never closed").is_err());
    }

    #[test]
    fn multiple_lifetimes_are_not_swallowed_as_character_literals() {
        let tokens = tokenize("fn borrow<'a, 'b>(left: &'a str, right: &'b str) {}")
            .expect("tokenize lifetimes");

        assert_eq!(
            tokens.iter().filter(|token| token.text == "'").count(),
            4,
            "each lifetime apostrophe remains punctuation"
        );
        assert_eq!(tokens.iter().filter(|token| token.text == "a").count(), 2);
        assert_eq!(tokens.iter().filter(|token| token.text == "b").count(), 2);
        assert!(!tokens.iter().any(|token| token.text == "<literal>"));
    }

    #[test]
    fn valid_character_literal_forms_are_single_literal_tokens() {
        let tokens = tokenize(
            r#"
                let ordinary = 'x';
                let escaped = '\n';
                let unicode_escape = '\u{1F600}';
                let unicode = '🦀';
                let byte = b'\xFF';
            "#,
        )
        .expect("tokenize character literals");

        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.text == "<literal>")
                .count(),
            5
        );
        assert!(!tokens.iter().any(|token| token.text == "'"));
        assert!(!tokens.iter().any(|token| token.text == "b"));
    }
}
