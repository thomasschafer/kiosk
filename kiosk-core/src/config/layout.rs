use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    Pane(Option<String>),
    Split {
        direction: Direction,
        children: Vec<Layout>,
    },
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Layout::Pane(None) => write!(f, "shell"),
            Layout::Pane(Some(cmd)) => {
                write!(f, "\"")?;
                for ch in cmd.chars() {
                    match ch {
                        '"' => write!(f, "\\\"")?,
                        '\\' => write!(f, "\\\\")?,
                        _ => write!(f, "{ch}")?,
                    }
                }
                write!(f, "\"")
            }
            Layout::Split {
                direction,
                children,
            } => {
                match direction {
                    Direction::Horizontal => write!(f, "h(")?,
                    Direction::Vertical => write!(f, "v(")?,
                }
                for (i, child) in children.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{child}")?;
                }
                write!(f, ")")
            }
        }
    }
}

impl Serialize for Layout {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub fn deserialize_layout<'de, D>(deserializer: D) -> Result<Option<Layout>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => parse_layout(&s).map(Some).map_err(serde::de::Error::custom),
    }
}

// --- Tokenizer ---

#[derive(Debug, PartialEq, Eq)]
enum Token {
    H,
    V,
    Shell,
    LParen,
    RParen,
    Comma,
    StringLit(String),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::H => write!(f, "'h'"),
            Token::V => write!(f, "'v'"),
            Token::Shell => write!(f, "'shell'"),
            Token::LParen => write!(f, "'('"),
            Token::RParen => write!(f, "')'"),
            Token::Comma => write!(f, "','"),
            Token::StringLit(s) => write!(f, "\"{s}\""),
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, LayoutParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            ',' => {
                tokens.push(Token::Comma);
                chars.next();
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err(LayoutParseError::UnterminatedString),
                        Some('\\') => match chars.next() {
                            Some(escaped) => s.push(escaped),
                            None => return Err(LayoutParseError::UnterminatedString),
                        },
                        Some('"') => break,
                        Some(c) => s.push(c),
                    }
                }
                tokens.push(Token::StringLit(s));
            }
            _ if ch.is_alphabetic() => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        word.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match word.as_str() {
                    "h" => tokens.push(Token::H),
                    "v" => tokens.push(Token::V),
                    "shell" => tokens.push(Token::Shell),
                    _ => {
                        return Err(LayoutParseError::UnknownKeyword(word));
                    }
                }
            }
            _ => {
                return Err(LayoutParseError::UnexpectedChar(ch));
            }
        }
    }

    Ok(tokens)
}

// --- Parser ---

#[derive(Debug, PartialEq, Eq)]
pub enum LayoutParseError {
    Empty,
    UnterminatedString,
    UnknownKeyword(String),
    UnexpectedChar(char),
    UnexpectedToken(String),
    UnexpectedEnd,
    SingleChild,
    TrailingTokens(String),
}

impl fmt::Display for LayoutParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutParseError::Empty => write!(f, "layout is empty"),
            LayoutParseError::UnterminatedString => write!(f, "unterminated string literal"),
            LayoutParseError::UnknownKeyword(w) => write!(f, "unknown keyword '{w}'"),
            LayoutParseError::UnexpectedChar(c) => write!(f, "unexpected character '{c}'"),
            LayoutParseError::UnexpectedToken(t) => write!(f, "unexpected token {t}"),
            LayoutParseError::UnexpectedEnd => write!(f, "unexpected end of input"),
            LayoutParseError::SingleChild => {
                write!(f, "split must have at least 2 children")
            }
            LayoutParseError::TrailingTokens(t) => {
                write!(f, "unexpected token {t} after layout")
            }
        }
    }
}

impl std::error::Error for LayoutParseError {}

pub fn parse_layout(input: &str) -> Result<Layout, LayoutParseError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(LayoutParseError::Empty);
    }
    let (layout, pos) = parse_node(&tokens, 0)?;
    if pos < tokens.len() {
        return Err(LayoutParseError::TrailingTokens(tokens[pos].to_string()));
    }
    Ok(layout)
}

fn parse_node(tokens: &[Token], pos: usize) -> Result<(Layout, usize), LayoutParseError> {
    let token = tokens.get(pos).ok_or(LayoutParseError::UnexpectedEnd)?;

    match token {
        Token::Shell => Ok((Layout::Pane(None), pos + 1)),
        Token::StringLit(s) => Ok((Layout::Pane(Some(s.clone())), pos + 1)),
        Token::H => parse_split(Direction::Horizontal, tokens, pos + 1),
        Token::V => parse_split(Direction::Vertical, tokens, pos + 1),
        other => Err(LayoutParseError::UnexpectedToken(other.to_string())),
    }
}

fn parse_split(
    direction: Direction,
    tokens: &[Token],
    pos: usize,
) -> Result<(Layout, usize), LayoutParseError> {
    let token = tokens.get(pos).ok_or(LayoutParseError::UnexpectedEnd)?;
    if *token != Token::LParen {
        return Err(LayoutParseError::UnexpectedToken(token.to_string()));
    }
    let mut pos = pos + 1;
    let mut children = Vec::new();

    loop {
        let (child, next_pos) = parse_node(tokens, pos)?;
        children.push(child);
        pos = next_pos;

        let token = tokens.get(pos).ok_or(LayoutParseError::UnexpectedEnd)?;
        match token {
            Token::Comma => {
                pos += 1;
                // Allow trailing comma before closing paren
                if tokens.get(pos) == Some(&Token::RParen) {
                    pos += 1;
                    break;
                }
            }
            Token::RParen => {
                pos += 1;
                break;
            }
            other => {
                return Err(LayoutParseError::UnexpectedToken(other.to_string()));
            }
        }
    }

    if children.len() < 2 {
        return Err(LayoutParseError::SingleChild);
    }

    Ok((
        Layout::Split {
            direction,
            children,
        },
        pos,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shell() {
        assert_eq!(parse_layout("shell").unwrap(), Layout::Pane(None));
    }

    #[test]
    fn parse_command() {
        assert_eq!(
            parse_layout("\"hx\"").unwrap(),
            Layout::Pane(Some("hx".to_string()))
        );
    }

    #[test]
    fn parse_command_with_args() {
        assert_eq!(
            parse_layout("\"claude --resume\"").unwrap(),
            Layout::Pane(Some("claude --resume".to_string()))
        );
    }

    #[test]
    fn parse_simple_horizontal_split() {
        assert_eq!(
            parse_layout("h(\"hx\", shell)").unwrap(),
            Layout::Split {
                direction: Direction::Horizontal,
                children: vec![Layout::Pane(Some("hx".to_string())), Layout::Pane(None),],
            }
        );
    }

    #[test]
    fn parse_simple_vertical_split() {
        assert_eq!(
            parse_layout("v(shell, \"claude\")").unwrap(),
            Layout::Split {
                direction: Direction::Vertical,
                children: vec![Layout::Pane(None), Layout::Pane(Some("claude".to_string())),],
            }
        );
    }

    #[test]
    fn parse_nested_split() {
        assert_eq!(
            parse_layout("h(v(shell, \"claude\"), \"hx\")").unwrap(),
            Layout::Split {
                direction: Direction::Horizontal,
                children: vec![
                    Layout::Split {
                        direction: Direction::Vertical,
                        children: vec![
                            Layout::Pane(None),
                            Layout::Pane(Some("claude".to_string())),
                        ],
                    },
                    Layout::Pane(Some("hx".to_string())),
                ],
            }
        );
    }

    #[test]
    fn parse_three_children() {
        assert_eq!(
            parse_layout("h(\"hx\", shell, \"claude\")").unwrap(),
            Layout::Split {
                direction: Direction::Horizontal,
                children: vec![
                    Layout::Pane(Some("hx".to_string())),
                    Layout::Pane(None),
                    Layout::Pane(Some("claude".to_string())),
                ],
            }
        );
    }

    #[test]
    fn parse_deeply_nested() {
        assert_eq!(
            parse_layout("h(v(h(shell, shell), \"claude\"), \"hx\")").unwrap(),
            Layout::Split {
                direction: Direction::Horizontal,
                children: vec![
                    Layout::Split {
                        direction: Direction::Vertical,
                        children: vec![
                            Layout::Split {
                                direction: Direction::Horizontal,
                                children: vec![Layout::Pane(None), Layout::Pane(None)],
                            },
                            Layout::Pane(Some("claude".to_string())),
                        ],
                    },
                    Layout::Pane(Some("hx".to_string())),
                ],
            }
        );
    }

    #[test]
    fn parse_escaped_quotes() {
        assert_eq!(
            parse_layout(r#""echo \"hello\"""#).unwrap(),
            Layout::Pane(Some("echo \"hello\"".to_string()))
        );
    }

    #[test]
    fn parse_escaped_backslash() {
        assert_eq!(
            parse_layout(r#""path\\to\\file""#).unwrap(),
            Layout::Pane(Some("path\\to\\file".to_string()))
        );
    }

    #[test]
    fn parse_whitespace_variations() {
        let expected = Layout::Split {
            direction: Direction::Horizontal,
            children: vec![Layout::Pane(Some("hx".to_string())), Layout::Pane(None)],
        };
        assert_eq!(parse_layout("h(\"hx\",shell)").unwrap(), expected);
        assert_eq!(parse_layout("  h( \"hx\" , shell )  ").unwrap(), expected);
        assert_eq!(parse_layout("h(\n\"hx\",\nshell\n)").unwrap(), expected);
    }

    #[test]
    fn error_empty() {
        assert_eq!(parse_layout(""), Err(LayoutParseError::Empty));
        assert_eq!(parse_layout("   "), Err(LayoutParseError::Empty));
    }

    #[test]
    fn error_unterminated_string() {
        assert_eq!(
            parse_layout("\"hello"),
            Err(LayoutParseError::UnterminatedString)
        );
    }

    #[test]
    fn error_unknown_keyword() {
        assert_eq!(
            parse_layout("foo"),
            Err(LayoutParseError::UnknownKeyword("foo".to_string()))
        );
    }

    #[test]
    fn error_unexpected_char() {
        assert_eq!(
            parse_layout("@"),
            Err(LayoutParseError::UnexpectedChar('@'))
        );
    }

    #[test]
    fn error_single_child() {
        assert_eq!(parse_layout("h(shell)"), Err(LayoutParseError::SingleChild));
    }

    #[test]
    fn error_trailing_tokens() {
        assert_eq!(
            parse_layout("shell shell"),
            Err(LayoutParseError::TrailingTokens("'shell'".to_string()))
        );
    }

    #[test]
    fn error_missing_lparen() {
        assert_eq!(
            parse_layout("h shell"),
            Err(LayoutParseError::UnexpectedToken("'shell'".to_string()))
        );
    }

    #[test]
    fn error_missing_rparen() {
        assert_eq!(
            parse_layout("h(shell, shell"),
            Err(LayoutParseError::UnexpectedEnd)
        );
    }

    #[test]
    fn parse_trailing_comma() {
        let expected = parse_layout("h(\"hx\", shell)").unwrap();
        assert_eq!(parse_layout("h(\"hx\", shell,)").unwrap(), expected);
    }

    #[test]
    fn parse_trailing_comma_three_children() {
        let expected = parse_layout("h(\"hx\", shell, \"claude\")").unwrap();
        assert_eq!(
            parse_layout("h(\"hx\", shell, \"claude\",)").unwrap(),
            expected
        );
    }

    #[test]
    fn parse_trailing_comma_nested() {
        let expected = parse_layout("h(v(shell, \"claude\"), \"hx\")").unwrap();
        assert_eq!(
            parse_layout("h(v(shell, \"claude\",), \"hx\",)").unwrap(),
            expected
        );
    }

    #[test]
    fn display_roundtrip() {
        let cases = [
            "shell",
            "\"hx\"",
            "h(\"hx\", shell)",
            "v(shell, \"claude\")",
            "h(v(shell, \"claude\"), \"hx\")",
            "h(\"hx\", shell, \"claude\")",
        ];
        for input in cases {
            let layout = parse_layout(input).unwrap();
            let displayed = layout.to_string();
            let reparsed = parse_layout(&displayed).unwrap();
            assert_eq!(layout, reparsed, "roundtrip failed for: {input}");
        }
    }

    #[test]
    fn display_escaped_roundtrip() {
        let layout = Layout::Pane(Some("echo \"hello\"".to_string()));
        let displayed = layout.to_string();
        let reparsed = parse_layout(&displayed).unwrap();
        assert_eq!(layout, reparsed);
    }
}
