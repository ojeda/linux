// SPDX-License-Identifier: GPL-2.0

//! JSON parser used to parse rustdoc output when retrieving doctests.

use std::collections::HashMap;
use std::iter::Peekable;
use std::str::FromStr;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JsonValue {
    Object(HashMap<String, JsonValue>),
    String(String),
    Number(i32),
    Bool(bool),
    Array(Vec<JsonValue>),
    Null,
}

fn parse_ident<I: Iterator<Item = char>>(
    iter: &mut I,
    output: JsonValue,
    ident: &str,
) -> Result<JsonValue, String> {
    let mut ident_iter = ident.chars().skip(1);

    loop {
        let i = ident_iter.next();
        if i.is_none() {
            return Ok(output);
        }
        let c = iter.next();
        if i != c {
            if let Some(c) = c {
                return Err(format!("Unexpected character `{c}` when parsing `{ident}`"));
            }
            return Err(format!("Missing character when parsing `{ident}`"));
        }
    }
}

fn parse_string<I: Iterator<Item = char>>(iter: &mut I) -> Result<JsonValue, String> {
    let mut out = String::new();

    while let Some(c) = iter.next() {
        match c {
            '\\' => {
                let Some(c) = iter.next() else { break };
                match c {
                    '"' | '\\' | '/' => out.push(c),
                    'b' => out.push(char::from(0x8u8)),
                    'f' => out.push(char::from(0xCu8)),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'n' => out.push('\n'),
                    _ => {
                        // This code doesn't handle codepoints so we put the string content as is.
                        out.push('\\');
                        out.push(c);
                    }
                }
            }
            '"' => {
                return Ok(JsonValue::String(out));
            }
            _ => out.push(c),
        }
    }
    Err(format!("Unclosed JSON string `{out}`"))
}

fn parse_number<I: Iterator<Item = char>>(
    iter: &mut Peekable<I>,
    digit: char,
) -> Result<JsonValue, String> {
    let mut nb = String::new();

    nb.push(digit);
    loop {
        // We peek next character to prevent taking it from the iterator in case it's a comma.
        if matches!(iter.peek(), Some(',' | '}' | ']')) {
            break;
        }
        let Some(c) = iter.next() else { break };
        if c.is_whitespace() {
            break;
        } else if !c.is_ascii_digit() {
            return Err(format!("Error when parsing number `{nb}`: found `{c}`"));
        }
        nb.push(c);
    }
    i32::from_str(&nb)
        .map(|nb| JsonValue::Number(nb))
        .map_err(|error| format!("Invalid number: `{error}`"))
}

fn parse_array<I: Iterator<Item = char>>(iter: &mut Peekable<I>) -> Result<JsonValue, String> {
    let mut values = Vec::new();

    'main: loop {
        let Some(c) = iter.next() else {
            return Err("Unclosed array".to_string());
        };
        if c.is_whitespace() {
            continue;
        } else if c == ']' {
            break;
        }
        values.push(parse(iter, c)?);
        while let Some(c) = iter.next() {
            if c.is_whitespace() {
                continue;
            } else if c == ',' {
                break;
            } else if c == ']' {
                break 'main;
            } else {
                return Err(format!("Unexpected `{c}` when parsing array"));
            }
        }
    }
    Ok(JsonValue::Array(values))
}

fn parse_object<I: Iterator<Item = char>>(iter: &mut Peekable<I>) -> Result<JsonValue, String> {
    let mut values = HashMap::new();

    'main: loop {
        let Some(c) = iter.next() else {
            return Err("Unclosed object".to_string());
        };
        let key;
        if c.is_whitespace() {
            continue;
        } else if c == '"' {
            let JsonValue::String(k) = parse_string(iter)? else {
                unreachable!()
            };
            key = k;
        } else if c == '}' {
            break;
        } else {
            return Err(format!("Expected `\"` when parsing Object, found `{c}`"));
        }

        // We then get the `:` separator.
        loop {
            let Some(c) = iter.next() else {
                return Err(format!("Missing value after key `{key}`"));
            };
            if c.is_whitespace() {
                continue;
            } else if c == ':' {
                break;
            } else {
                return Err(format!(
                    "Expected `:` after key, found `{c}` when parsing object"
                ));
            }
        }
        // Then the value.
        let value = loop {
            let Some(c) = iter.next() else {
                return Err(format!("Missing value after key `{key}`"));
            };
            if c.is_whitespace() {
                continue;
            } else {
                break parse(iter, c)?;
            }
        };

        if values.contains_key(&key) {
            return Err(format!("Duplicated key `{key}`"));
        }
        values.insert(key, value);

        while let Some(c) = iter.next() {
            if c.is_whitespace() {
                continue;
            } else if c == ',' {
                break;
            } else if c == '}' {
                break 'main;
            } else {
                return Err(format!("Unexpected `{c}` when parsing array"));
            }
        }
    }
    Ok(JsonValue::Object(values))
}

fn parse<I: Iterator<Item = char>>(iter: &mut Peekable<I>, c: char) -> Result<JsonValue, String> {
    match c {
        '{' => parse_object(iter),
        '"' => parse_string(iter),
        '[' => parse_array(iter),
        't' => parse_ident(iter, JsonValue::Bool(true), "true"),
        'f' => parse_ident(iter, JsonValue::Bool(false), "false"),
        'n' => parse_ident(iter, JsonValue::Null, "null"),
        c => {
            if c.is_ascii_digit() || c == '-' {
                parse_number(iter, c)
            } else {
                Err(format!("Unexpected `{c}` character"))
            }
        }
    }
}

impl JsonValue {
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let mut iter = input.chars().peekable();
        let mut value = None;

        while let Some(c) = iter.next() {
            if c.is_whitespace() {
                continue;
            }
            value = Some(parse(&mut iter, c)?);
            break;
        }
        while let Some(c) = iter.next() {
            if c.is_whitespace() {
                continue;
            } else {
                return Err(format!("Unexpected character `{c}` after content"));
            }
        }
        if let Some(value) = value {
            Ok(value)
        } else {
            Err("Empty content".to_string())
        }
    }
}
