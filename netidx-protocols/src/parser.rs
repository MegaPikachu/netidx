use crate::view::{Sink, Source};
use base64;
use bytes::Bytes;
use combine::{
    attempt, between, choice, from_str, many1, optional,
    parser::{
        char::{digit, spaces, string},
        combinator::recognize,
        range::{take_while, take_while1},
        repeat::escaped,
    },
    sep_by1,
    stream::{position, Range},
    token, EasyParser, ParseError, Parser, RangeStream,
};
use netidx::{chars::Chars, path::Path, publisher::Value};
use std::{result::Result, str::FromStr};

fn unescape(s: String, esc: char) -> String {
    if !s.contains(esc) {
        s
    } else {
        let mut res = String::with_capacity(s.len());
        let mut escaped = false;
        res.extend(s.chars().filter_map(|c| {
            if c == esc && !escaped {
                escaped = true;
                None
            } else {
                escaped = false;
                Some(c)
            }
        }));
        res
    }
}

fn escaped_string<I>(cq: char) -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    recognize(escaped(take_while1(move |c| c != cq && c != '\\'), '\\', token(cq)))
        .map(|s| unescape(s, '\\'))
}

fn quoted<I>(oq: char, cq: char) -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    spaces().with(between(token(oq), token(cq), escaped_string(cq)))
}

fn uint<I>() -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    many1(digit())
}

fn int<I>() -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    recognize((optional(token('-')), take_while1(|c: char| c.is_digit(10))))
}

fn flt<I>() -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    recognize((digit(), optional(token('.')), take_while(|c: char| c.is_digit(10))))
}

struct Base64Encoded(Vec<u8>);

impl FromStr for Base64Encoded {
    type Err = base64::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        base64::decode(s).map(Base64Encoded)
    }
}

fn base64str<I>() -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    recognize((
        take_while(|c: char| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
        take_while(|c: char| c == '='),
    ))
}

fn fname<I>() -> impl Parser<I, Output = String>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    recognize((
        take_while1(|c: char| c.is_alphabetic() && c.is_lowercase()),
        take_while(|c: char| (c.is_alphanumeric() && c.is_lowercase()) || c == '_'),
    ))
}

fn constant<I>(typ: &'static str) -> impl Parser<I, Output = char>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    string("constant")
        .with(spaces())
        .with(token('('))
        .with(spaces())
        .with(string(typ))
        .with(spaces())
        .with(token(','))
}

fn source_<I>() -> impl Parser<I, Output = Source>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    spaces().with(choice((
        attempt(
            constant("u32")
                .with(spaces().with(from_str(uint())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::U32(v))),
        ),
        attempt(
            constant("v32")
                .with(spaces().with(from_str(uint())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::V32(v))),
        ),
        attempt(
            constant("i32")
                .with(spaces().with(from_str(int())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::I32(v))),
        ),
        attempt(
            constant("z32")
                .with(spaces().with(from_str(int())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::Z32(v))),
        ),
        attempt(
            constant("u64")
                .with(spaces().with(from_str(uint())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::U64(v))),
        ),
        attempt(
            constant("v64")
                .with(spaces().with(from_str(uint())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::V64(v))),
        ),
        attempt(
            constant("i64")
                .with(spaces().with(from_str(int())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::I64(v))),
        ),
        attempt(
            constant("z64")
                .with(spaces().with(from_str(int())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::Z64(v))),
        ),
        attempt(
            constant("f32")
                .with(spaces().with(from_str(flt())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::F32(v))),
        ),
        attempt(
            constant("f64")
                .with(spaces().with(from_str(flt())))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::F64(v))),
        ),
        attempt(
            constant("string")
                .with(escaped_string(')'))
                .and(spaces().with(token(')')))
                .map(|(v, _)| Source::Constant(Value::String(Chars::from(v)))),
        ),
        attempt(
            constant("bytes")
                .with(from_str(base64str()))
                .and(spaces().with(token(')')))
                .map(|(Base64Encoded(v), _)| {
                    Source::Constant(Value::Bytes(Bytes::from(v)))
                }),
        ),
        attempt(
            constant("bool")
                .with(spaces().with(string("true")))
                .and(spaces().with(token(')')))
                .map(|(_, _)| Source::Constant(Value::True)),
        ),
        attempt(
            constant("bool")
                .with(spaces().with(string("false")))
                .and(spaces().with(token(')')))
                .map(|(_, _)| Source::Constant(Value::False)),
        ),
        attempt(
            string("constant")
                .with(spaces())
                .with(between(
                    token('('),
                    token(')'),
                    spaces().with(string("null")).with(spaces()),
                ))
                .map(|_| Source::Constant(Value::Null)),
        ),
        attempt(
            constant("result")
                .with(spaces().with(string("ok")))
                .and(spaces().with(token(')')))
                .map(|(_, _)| Source::Constant(Value::Ok)),
        ),
        attempt(
            constant("result")
                .with(escaped_string(')'))
                .and(spaces().with(token(')')))
                .map(|(s, _)| Source::Constant(Value::Error(Chars::from(s)))),
        ),
        attempt(
            string("load_path")
                .with(quoted('(', ')'))
                .map(|s| Source::Load(Path::from(s))),
        ),
        attempt(
            string("load_var")
                .with(between(
                    spaces().with(token('(')),
                    spaces().with(token(')')),
                    spaces().with(fname()),
                ))
                .map(|s| Source::Variable(s)),
        ),
        (
            fname(),
            between(
                spaces().with(token('(')),
                spaces().with(token(')')),
                spaces().with(sep_by1(source(), spaces().with(token(',')))),
            ),
        )
            .map(|(function, from)| Source::Map { function, from }),
    )))
}

parser! {
    fn source[I]()(I) -> Source
    where [I: RangeStream<Token = char>, I::Range: Range]
    {
        source_()
    }
}

pub fn parse_source(s: &str) -> anyhow::Result<Source> {
    source()
        .easy_parse(position::Stream::new(s))
        .map(|(r, _)| r)
        .map_err(|e| anyhow::anyhow!(format!("{}", e)))
}

fn sink_<I>() -> impl Parser<I, Output = Sink>
where
    I: RangeStream<Token = char>,
    I::Error: ParseError<I::Token, I::Range, I::Position>,
    I::Range: Range,
{
    spaces().with(choice((
        attempt(
            string("store_path")
                .with(quoted('(', ')'))
                .map(|s| Sink::Store(Path::from(s))),
        ),
        attempt(
            string("store_var")
                .with(between(
                    spaces().with(token('(')),
                    spaces().with(token(')')),
                    spaces().with(fname()),
                ))
                .map(|s| Sink::Variable(s)),
        ),
        attempt(
            (
                fname(),
                between(
                    spaces().with(token('(')),
                    spaces().with(token(')')),
                    spaces().with(sep_by1(sink(), spaces().with(token(',')))),
                ),
            )
                .map(|(function, from)| Sink::Map { function, from }),
        ),
    )))
}

parser! {
    fn sink[I]()(I) -> Sink
    where [I: RangeStream<Token = char>, I::Range: Range]
    {
        sink_()
    }
}

pub fn parse_sink(s: &str) -> anyhow::Result<Sink> {
    sink()
        .easy_parse(position::Stream::new(s))
        .map(|(r, _)| r)
        .map_err(|e| anyhow::anyhow!(format!("{}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_parse() {
        let p = Path::from(r#"/foo bar baz/(zam)/_ xyz+ "#);
        let s = r#"store_path(/foo bar baz/(zam\)_ xyz+ )"#;
        assert_eq!(Sink::Store(p), parse_sink(s).unwrap());
        assert_eq!(
            Sink::Variable(String::from("foo")),
            parse_sink("store_var(foo)").unwrap()
        );
        let snk = Sink::Map {
            from: vec![
                Sink::Store(Path::from("/foo/bar")),
                Sink::Variable(String::from("foo")),
            ],
            function: String::from("all"),
        };
        let chs = "all(store_path(/foo/bar), store_var(foo))";
        assert_eq!(snk, parse_sink(chs).unwrap());
    }

    #[test]
    fn source_parse() {
        assert_eq!(
            Source::Constant(Value::U32(23)),
            parse_source("constant( u32, 23 )").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::V32(42)),
            parse_source("constant(v32, 42)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::I32(-10)),
            parse_source("constant(i32, -10)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::I32(12321)),
            parse_source("i32:12321").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Z32(-99)),
            parse_source("constant(z32, -99)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::U64(100)),
            parse_source("constant(u64, 100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::V64(100)),
            parse_source("constant(v64, 100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::I64(-100)),
            parse_source("constant(i64, -100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::I64(100)),
            parse_source("constant(i64, 100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Z64(-100)),
            parse_source("constant(z64, -100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Z64(100)),
            parse_source("constant(z64, 100)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F32(3.1415)),
            parse_source("constant(f32, 3.1415)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F32(3.)),
            parse_source("constant(f32, 3)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F32(3.)),
            parse_source("constant(f32, 3.)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F64(3.1415)),
            parse_source("constant(f64, 3.1415)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F64(3.)),
            parse_source("constant(f64, 3.)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::F64(3.)),
            parse_source("constant(f64, 3)").unwrap()
        );
        let c = Chars::from(r#"I've got a lovely "bunch" of (coconuts)"#);
        let s = r#"constant(string, I've got a lovely "bunch" of (coconuts\))"#;
        assert_eq!(Source::Constant(Value::String(c)), parse_source(s).unwrap());
        assert_eq!(
            Source::Constant(Value::True),
            parse_source("constant(bool, true)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::False),
            parse_source("constant(bool, false)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Null),
            parse_source("constant(null)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Ok),
            parse_source("constant(result, ok)").unwrap()
        );
        assert_eq!(
            Source::Constant(Value::Error(Chars::from("error"))),
            parse_source("constant(result, error)").unwrap()
        );
        let p = Path::from(r#"/foo bar baz/"zam"/)_ xyz+ "#);
        let s = r#"load_path(/foo bar baz/\"zam\"/\)_ xyz+ )"#;
        assert_eq!(Source::Load(p), parse_source(s).unwrap());
        assert_eq!(
            Source::Variable(String::from("sum")),
            parse_source("load_var(sum)").unwrap()
        );
        let src = Source::Map {
            from: vec![
                Source::Constant(Value::F32(1.)),
                Source::Load(Path::from("/foo/bar")),
                Source::Map {
                    from: vec![
                        Source::Constant(Value::F32(0.)),
                        Source::Load(Path::from("/foo/baz")),
                    ],
                    function: String::from("max"),
                },
            ],
            function: String::from("sum"),
        };
        let chs = r#"sum(constant(f32, 1), load_path(/foo/bar), max(constant(f32, 0), load_path(/foo/baz)))"#;
        assert_eq!(src, parse_source(chs).unwrap());
    }
}
