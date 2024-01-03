#![allow(clippy::needless_borrowed_reference)]

//!
//! input := name '->' jmespath '<' type '>' context , "description"

use core::str::FromStr;

use serde::{Deserialize, Serialize};

///
struct Walker<'inner, T> {
    ///
    buffer: &'inner [T],

    ///
    current: usize,
}

impl<'inner, T: PartialEq> Walker<'inner, T> {
    ///
    fn new(buffer: &'inner [T]) -> Self {
        Self { buffer, current: 0 }
    }

    ///
    fn peek(&self) -> Option<&T> {
        self.buffer.get(self.current)
    }

    ///
    fn match_tokens(&mut self, tokens: &[T]) -> bool {
        let mut current = self.current;

        for token in tokens {
            if let Some(next) = self.buffer.get(current) {
                if next != token {
                    return false;
                }
            } else {
                return false;
            }

            current = current.saturating_add(1);
        }
        self.current = self.current.saturating_add(tokens.len());

        true
    }

    ///
    fn advance(&mut self) {
        self.current = self.current.saturating_add(1);
    }

    ///
    fn buffer_size(&self) -> usize {
        self.buffer.len()
    }
}

///
#[derive(Debug, Clone, PartialEq, Eq)]
enum InputTokens {
    ///
    InputArrow,

    ///
    OutputArrow,

    ///
    Lt,

    ///
    Gt,

    ///
    Dot,

    ///
    LeftBracket,

    ///
    RightBracket,

    ///
    Identifier(String),

    ///
    Integer(i64),

    ///
    String(String),
}

///
fn lexer<T: AsRef<[u8]>>(input: T) -> anyhow::Result<Vec<InputTokens>> {
    let mut walker = Walker::new(input.as_ref());
    let mut tokens = Vec::with_capacity(walker.buffer_size());

    while let Some(current) = walker.peek().copied() {
        match current {
            b'<' => {
                walker.advance();
                if walker.match_tokens(&[b'-']) {
                    tokens.push(InputTokens::OutputArrow);
                } else {
                    tokens.push(InputTokens::Lt);
                }
            }
            b'>' => {
                walker.advance();
                tokens.push(InputTokens::Gt);
            }
            b'.' => {
                walker.advance();
                tokens.push(InputTokens::Dot);
            }
            b'[' => {
                walker.advance();
                tokens.push(InputTokens::LeftBracket);
            }
            b']' => {
                walker.advance();
                tokens.push(InputTokens::RightBracket);
            }
            b'-' => {
                walker.advance();
                if walker.match_tokens(&[b'>']) {
                    tokens.push(InputTokens::InputArrow);
                } else {
                    return Err(anyhow::anyhow!(
                        "[Invalid Token]: Expected input arrow (->)"
                    ));
                }
            }
            b'"' => {
                walker.advance();
                let mut string = Vec::new();
                while let Some(current) = walker.peek().copied() {
                    if current == b'"' {
                        walker.advance();
                        break;
                    }

                    string.push(current);
                    walker.advance();
                }

                let string = String::from_utf8(string)?;
                tokens.push(InputTokens::String(string));
            }
            b' ' | b'\t' | b'\n' => {
                walker.advance();
            }
            b'0'..=b'9' => {
                let mut integer = Vec::new();
                while let Some(current) = walker.peek().copied() {
                    if !current.is_ascii_digit() {
                        break;
                    }

                    integer.push(current);
                    walker.advance();
                }

                let integer = String::from_utf8(integer)?;
                let integer = integer.parse::<i64>()?;
                tokens.push(InputTokens::Integer(integer));
            }
            _ => {
                let mut identifier = Vec::new();
                while let Some(current) = walker.peek().copied() {
                    if current == b' '
                        || current == b'\t'
                        || current == b'\n'
                        || current == b'.'
                        || current == b'['
                        || current == b']'
                        || current == b'<'
                        || current == b'>'
                        || current == b'"'
                        || current == b'-'
                    {
                        break;
                    }

                    identifier.push(current);
                    walker.advance();
                }

                let identifier = String::from_utf8(identifier)?;
                tokens.push(InputTokens::Identifier(identifier));
            }
        }
    }

    Ok(tokens)
}

///
fn parse(input: &[InputTokens]) -> anyhow::Result<InputDescription> {
    let mut walker = Walker::new(input);

    let name = parse_name(&mut walker)?;

    let direction = match walker.peek() {
        Some(&InputTokens::InputArrow) => Direction::Input,
        Some(&InputTokens::OutputArrow) => Direction::Output,
        _ => return Err(anyhow::anyhow!("Invalid arrow token")),
    };
    walker.advance();

    let (path, raw_path) = parse_path(&mut walker)?;

    let input_type = parse_input_type(&mut walker)?;

    Ok(InputDescription {
        direction,
        name,
        path,
        raw_path,
        input_type,
    })
}

///
fn parse_name(walker: &mut Walker<InputTokens>) -> anyhow::Result<String> {
    if let Some(&InputTokens::Identifier(ref name)) = walker.peek() {
        let name = name.clone();
        walker.advance();
        Ok(name)
    } else {
        Err(anyhow::anyhow!(
            "[Parse Error] Expected name Identifier, found: {:?}",
            walker.peek()
        ))
    }
}

///
fn parse_path(walker: &mut Walker<InputTokens>) -> anyhow::Result<(Vec<PathKey>, String)> {
    let mut path = Vec::new();
    let mut raw_path = String::new();

    if let Some(&InputTokens::LeftBracket) = walker.peek() {
        walker.advance();
        let key = parse_integer_key(walker)?;

        if let Some(&InputTokens::RightBracket) = walker.peek() {
            walker.advance();

            raw_path.push('[');
            raw_path.push_str(&key.to_string());
            raw_path.push(']');

            path.push(key);
        } else {
            return Err(anyhow::anyhow!(
                "[Parse Error]: Expected closing bracket ']'"
            ));
        }
    } else {
        let first_key = parse_string_key(walker)?;

        raw_path.push_str(&first_key.to_string());
        path.push(first_key);
    }

    loop {
        match walker.peek() {
            Some(&InputTokens::Dot) => {
                walker.advance();

                let key = parse_string_key(walker)?;
                raw_path.push('.');
                raw_path.push_str(&key.to_string());

                path.push(key);
            }
            Some(&InputTokens::LeftBracket) => {
                walker.advance();
                let key = parse_integer_key(walker)?;

                if let Some(&InputTokens::RightBracket) = walker.peek() {
                    walker.advance();

                    raw_path.push('[');
                    raw_path.push_str(&key.to_string());
                    raw_path.push(']');

                    path.push(key);
                } else {
                    return Err(anyhow::anyhow!(
                        "[Parse Error]: Expected closing bracket ']'"
                    ));
                }
            }
            _ => break,
        }
    }

    Ok((path, raw_path))
}

///
fn parse_string_key(walker: &mut Walker<InputTokens>) -> anyhow::Result<PathKey> {
    if let Some(&InputTokens::Identifier(ref key)) = walker.peek() {
        let key = key.clone();
        walker.advance();
        Ok(PathKey::Identifier(key))
    } else {
        Err(anyhow::anyhow!(
            "[Parse Error]: Expected key Identifier, found: {:?}",
            walker.peek()
        ))
    }
}

///
fn parse_integer_key(walker: &mut Walker<InputTokens>) -> anyhow::Result<PathKey> {
    match walker.peek() {
        Some(&InputTokens::Integer(key)) => {
            walker.advance();
            Ok(PathKey::Integer(key))
        }
        Some(&InputTokens::String(ref key)) => {
            let key = key.clone();
            walker.advance();
            Ok(PathKey::String(key))
        }
        _ => Err(anyhow::anyhow!(
            "[Parse Error]: Expected Integer or String, found: {:?}",
            walker.peek()
        )),
    }
}

///
fn parse_input_type(walker: &mut Walker<InputTokens>) -> anyhow::Result<InputType> {
    if let Some(&InputTokens::Lt) = walker.peek() {
        walker.advance();
    } else {
        return Err(anyhow::anyhow!(
            "[Parse Error]: Expected opening bracket '<'"
        ));
    }

    let input_type = if let Some(&InputTokens::Identifier(ref input_type)) = walker.peek() {
        let input_type = match input_type.to_lowercase().as_str() {
            "string" => InputType::String,
            "integer" => InputType::Integer,
            "number" => InputType::Number,
            "boolean" => InputType::Boolean,
            "object" => InputType::Object,
            "array" => InputType::Array,
            "null" => InputType::Null,
            _ => {
                return Err(anyhow::anyhow!(
                    "[Parse Error]: Expected valid type, found: [{}]",
                    input_type
                ))
            }
        };

        walker.advance();
        input_type
    } else {
        return Err(anyhow::anyhow!(
            "[Parse Error]: Expected type Identifier, found: {:?}",
            walker.peek()
        ));
    };

    if let Some(&InputTokens::Gt) = walker.peek() {
        walker.advance();
    } else {
        return Err(anyhow::anyhow!(
            "[Parse Error]: Expected closing bracket '>'"
        ));
    }

    Ok(input_type)
}

///
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum PathKey {
    ///
    String(String),

    ///
    Identifier(String),

    ///
    Integer(i64),
}

impl ToString for PathKey {
    fn to_string(&self) -> String {
        match self {
            &PathKey::Identifier(ref key) => key.to_string(),
            &PathKey::Integer(ref key) => key.to_string(),
            &PathKey::String(ref key) => format!("\"{key}\""),
        }
    }
}

///
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum InputType {
    ///
    String,

    ///
    Integer,

    ///
    Number,

    ///
    Boolean,

    ///
    Object,

    ///
    Array,

    ///
    Null,
}

///
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    ///
    Input,
    ///
    Output,
}

///
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDescription {
    ///
    pub direction: Direction,

    ///
    pub name: String,

    ///
    pub path: Vec<PathKey>,

    ///
    pub raw_path: String,

    ///
    pub input_type: InputType,
}

impl FromStr for InputDescription {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let tokens = lexer(s)?;
        parse(&tokens)
    }
}

#[cfg(test)]
mod test {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;

    #[test]
    fn test_lexer() -> anyhow::Result<()> {
        let input = "name -> $body.input[0].id <STRING> context , \"description\"";

        let expected = vec![
            InputTokens::Identifier("name".to_owned()),
            InputTokens::InputArrow,
            InputTokens::Identifier("$body".to_owned()),
            InputTokens::Dot,
            InputTokens::Identifier("input".to_owned()),
            InputTokens::LeftBracket,
            InputTokens::Integer(0),
            InputTokens::RightBracket,
            InputTokens::Dot,
            InputTokens::Identifier("id".to_owned()),
            InputTokens::Lt,
            InputTokens::Identifier("STRING".to_owned()),
            InputTokens::Gt,
            InputTokens::Identifier("context".to_owned()),
            InputTokens::Identifier(",".to_owned()),
            InputTokens::String("description".to_owned()),
        ];

        let tokens = lexer(input)?;

        assert_eq!(expected, tokens);

        Ok(())
    }

    #[test]
    fn test_parse() -> anyhow::Result<()> {
        let input = "name -> $body.input[0][\"id\"] <STRING> context , \"description\"";
        let input = input.parse::<InputDescription>()?;

        let expected = InputDescription {
            direction: Direction::Input,
            name: "name".to_owned(),
            path: vec![
                PathKey::Identifier("$body".to_owned()),
                PathKey::Identifier("input".to_owned()),
                PathKey::Integer(0),
                PathKey::String("id".to_owned()),
            ],
            raw_path: "$body.input[0][\"id\"]".to_owned(),
            input_type: InputType::String,
        };

        assert_eq!(expected, input);

        Ok(())
    }

    #[test]
    fn test_parse_output() -> anyhow::Result<()> {
        let input = "name <- $body.input[0][\"id\"] <STRING> context , \"description\"";
        let input = input.parse::<InputDescription>()?;

        let expected = InputDescription {
            direction: Direction::Output,
            name: "name".to_owned(),
            path: vec![
                PathKey::Identifier("$body".to_owned()),
                PathKey::Identifier("input".to_owned()),
                PathKey::Integer(0),
                PathKey::String("id".to_owned()),
            ],
            raw_path: "$body.input[0][\"id\"]".to_owned(),
            input_type: InputType::String,
        };

        assert_eq!(expected, input);

        Ok(())
    }

    #[test]
    fn test_parse_starts_with_index() -> anyhow::Result<()> {
        let input = "name <- [0].data <STRING> context , \"description\"";
        let input = input.parse::<InputDescription>()?;

        let expected = InputDescription {
            direction: Direction::Output,
            name: "name".to_owned(),
            path: vec![PathKey::Integer(0), PathKey::Identifier("data".to_owned())],
            raw_path: "[0].data".to_owned(),
            input_type: InputType::String,
        };

        assert_eq!(expected, input);

        Ok(())
    }
}
