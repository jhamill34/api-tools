#![allow(clippy::needless_borrowed_reference)]

//! A hand-rolled lexer/parser for the `Generate` command's small input/
//! output-mapping DSL:
//!
//! input := name '->' jmespath '<' type '>' context , "description"

use core::str::FromStr;

use serde::{Deserialize, Serialize};

/// A cursor over a slice, used by [`lexer`] (over bytes) and [`parse`]
/// (over tokens) to scan forward without indexing errors.
struct Walker<'inner, T> {
    /// The slice being scanned.
    buffer: &'inner [T],

    /// The index of the next unconsumed element.
    current: usize,
}

impl<'inner, T: PartialEq> Walker<'inner, T> {
    /// Creates a [`Walker`] positioned at the start of `buffer`.
    fn new(buffer: &'inner [T]) -> Self {
        Self { buffer, current: 0 }
    }

    /// Returns the next unconsumed element without advancing.
    fn peek(&self) -> Option<&T> {
        self.buffer.get(self.current)
    }

    /// If `tokens` matches starting at the current position, advances past
    /// them and returns `true`; otherwise leaves the position unchanged
    /// and returns `false`.
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

    /// Advances one element without reading it.
    fn advance(&mut self) {
        self.current = self.current.saturating_add(1);
    }

    /// The total length of the underlying buffer.
    fn buffer_size(&self) -> usize {
        self.buffer.len()
    }
}

/// A lexical token of the input/output-mapping DSL.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InputTokens {
    /// `->`, marking an input mapping.
    InputArrow,

    /// `<-`, marking an output mapping.
    OutputArrow,

    /// `<`, opening a type annotation.
    Lt,

    /// `>`, closing a type annotation.
    Gt,

    /// `.`, separating path segments.
    Dot,

    /// `[`, opening an indexed path segment.
    LeftBracket,

    /// `]`, closing an indexed path segment.
    RightBracket,

    /// A bare word: a name, path segment, type name, or context.
    Identifier(String),

    /// A numeric literal, used as an array index.
    Integer(i64),

    /// A double-quoted string literal.
    String(String),
}

/// Scans `input`'s bytes into a token stream, skipping whitespace.
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

/// Parses the `name '->'|'<-' path '<' type '>'` prefix of the DSL into an
/// [`InputDescription`]. [`InputDescription`] has no field for the
/// grammar's trailing `context , "description"`, so any tokens after the
/// type annotation are left unconsumed rather than erroring.
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

/// Consumes a leading identifier as the mapping's name.
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

/// Consumes a dotted/indexed path (e.g. `foo.bar[0]["baz"]`) into both a
/// structured [`PathKey`] list and its original string form.
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

/// Consumes an identifier as a dotted path segment (`.foo`).
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

/// Consumes an integer or string literal as a bracketed path segment
/// (`[0]` or `["key"]`).
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

/// Consumes a `<type>` annotation and resolves it to an [`InputType`].
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

/// One segment of a parsed field path.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum PathKey {
    /// A quoted bracketed key, e.g. `["key"]`.
    String(String),

    /// A dotted identifier segment, e.g. `.foo`.
    Identifier(String),

    /// A bracketed numeric index, e.g. `[0]`.
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

/// The JSON type a mapped field is expected to hold.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum InputType {
    /// A string value.
    String,

    /// An integer value.
    Integer,

    /// A floating-point value.
    Number,

    /// A boolean value.
    Boolean,

    /// An object value.
    Object,

    /// An array value.
    Array,

    /// A null value.
    Null,
}

/// Whether a mapping (`->`) feeds a field into the template's input, or
/// (`<-`) reads one from its output.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Maps a field into the template's input.
    Input,
    /// Maps a field out of the template's output.
    Output,
}

/// One parsed line of the input/output-mapping DSL.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDescription {
    /// Whether this maps an input or an output field.
    pub direction: Direction,

    /// The mapping's name.
    pub name: String,

    /// The field's path, as structured segments.
    pub path: Vec<PathKey>,

    /// The field's path, in its original string form.
    pub raw_path: String,

    /// The field's expected type.
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
