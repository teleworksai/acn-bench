//! The predicate language of HYP-10: a lexer and a recursive-descent parser for
//! the grammar as SPEC 080 writes it. Names are resolved against the file
//! (parameters, enum values, quantities) by [`crate::check`], which also types
//! the tree; this module only knows the syntax and the reserved words.

use std::fmt;

/// The built-in functions of HYP-13.
pub const BUILTINS: &[&str] = &[
    "abs",
    "min",
    "max",
    "effect",
    "rel_effect",
    "ci_low",
    "ci_high",
    "max_over_knobs",
    "min_over_knobs",
    "noise_floor",
];

/// HYP-10's reserved words: keywords, counters and the built-in names.
#[must_use]
pub fn is_reserved(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "or"
            | "not"
            | "at"
            | "all"
            | "any"
            | "cells"
            | "control"
            | "treatment"
            | "ci"
            | "true"
            | "false"
            | "replicates"
            | "providers_reported"
    ) || BUILTINS.contains(&word)
}

/// Whether `s` is an `ident` of the grammar: `[a-z_][a-z0-9_]*`.
#[must_use]
pub fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl CmpOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Eq => "==",
            Self::Ne => "!=",
        }
    }

    /// Apply to two numbers.
    #[must_use]
    pub fn apply(self, a: f64, b: f64) -> bool {
        match self {
            Self::Lt => a < b,
            Self::Le => a <= b,
            Self::Gt => a > b,
            Self::Ge => a >= b,
            Self::Eq => a == b,
            Self::Ne => a != b,
        }
    }
}

/// An arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }
}

/// The two counters (HYP-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    Replicates,
    ProvidersReported,
}

/// A literal value in a selector: `evalue | true | false | snumber`.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Ident(String),
    Bool(bool),
    Num(f64),
}

/// One selector of a `select` (HYP-12).
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    Control,
    Treatment,
    /// `pname = value`.
    Fix(String, Lit),
    /// A bare enum value.
    Value(String),
}

/// One argument of a built-in.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Ci(f64),
    Control,
    Treatment,
    /// An expression; a bare `qname` is also how a quantity reference is written.
    Expr(Expr),
}

/// The range of an `at` clause.
#[derive(Debug, Clone, PartialEq)]
pub enum AtRange {
    /// `pname cmpop snumber`.
    Bound(String, CmpOp, f64),
    Cells,
}

/// A predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Counter(Counter),
    /// A bare `qname`.
    Quantity(String),
    Select(String, Vec<Selector>),
    Call(String, Vec<Arg>),
    Neg(Box<Expr>),
    Arith(ArithOp, Box<Expr>, Box<Expr>),
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// The whole predicate, quantified over cells (`all` when true).
    At(Box<Expr>, bool, AtRange),
}

/// Why a predicate does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at {at}: {message}")]
pub struct ParseError {
    /// Byte offset in the predicate text.
    pub at: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    LParen,
    RParen,
    Comma,
    Assign,
    Cmp(CmpOp),
    Plus,
    Minus,
    Star,
    Slash,
    End,
}

fn lex(src: &str) -> Result<Vec<(usize, Tok)>, ParseError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let err = |at: usize, m: String| Err(ParseError { at, message: m });
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let two = b.get(i + 1).copied();
        let tok = match c {
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b',' => Tok::Comma,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'<' if two == Some(b'=') => {
                i += 1;
                Tok::Cmp(CmpOp::Le)
            }
            b'<' => Tok::Cmp(CmpOp::Lt),
            b'>' if two == Some(b'=') => {
                i += 1;
                Tok::Cmp(CmpOp::Ge)
            }
            b'>' => Tok::Cmp(CmpOp::Gt),
            b'=' if two == Some(b'=') => {
                i += 1;
                Tok::Cmp(CmpOp::Eq)
            }
            b'=' => Tok::Assign,
            b'!' if two == Some(b'=') => {
                i += 1;
                Tok::Cmp(CmpOp::Ne)
            }
            b'0'..=b'9' => {
                // number := [0-9]+ ( "." [0-9]+ )? ( [eE] [+-]? [0-9]+ )?
                let digits = |mut j: usize| {
                    let s = j;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    (j, j > s)
                };
                let (mut j, _) = digits(i);
                if j < b.len() && b[j] == b'.' {
                    let (k, any) = digits(j + 1);
                    if !any {
                        return err(j, "a `.` must be followed by digits".into());
                    }
                    j = k;
                }
                if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                    let mut k = j + 1;
                    if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                        k += 1;
                    }
                    let (k, any) = digits(k);
                    if !any {
                        return err(j, "an exponent needs digits".into());
                    }
                    j = k;
                }
                let text = &src[i..j];
                let v: f64 = text.parse().map_err(|_| ParseError {
                    at: i,
                    message: format!("`{text}` is not a number"),
                })?;
                if !v.is_finite() {
                    return err(i, format!("`{text}` is not finite"));
                }
                i = j - 1;
                Tok::Num(v)
            }
            b'a'..=b'z' | b'_' => {
                let mut j = i;
                while j < b.len()
                    && (b[j].is_ascii_lowercase() || b[j].is_ascii_digit() || b[j] == b'_')
                {
                    j += 1;
                }
                let t = Tok::Ident(src[i..j].to_owned());
                i = j - 1;
                t
            }
            _ => {
                return err(
                    i,
                    format!(
                        "unexpected character `{}`",
                        src[i..].chars().next().unwrap_or('?')
                    ),
                );
            }
        };
        out.push((start, tok));
        i += 1;
    }
    out.push((src.len(), Tok::End));
    Ok(out)
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].1
    }

    fn peek2(&self) -> &Tok {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)].1
    }

    fn at(&self) -> usize {
        self.toks[self.pos.min(self.toks.len() - 1)].0
    }

    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn fail<T>(&self, m: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            at: self.at(),
            message: m.into(),
        })
    }

    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), ParseError> {
        if self.peek() == t {
            self.bump();
            Ok(())
        } else {
            self.fail(format!("expected {what}, found {}", describe(self.peek())))
        }
    }

    fn keyword(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == k)
    }

    /// expr := or ( "at" ( "all" | "any" ) ( pname cmpop snumber | "cells" ) )?
    fn expr(&mut self) -> Result<Expr, ParseError> {
        let e = self.or()?;
        if self.keyword("at") {
            self.bump();
            let all = match self.bump() {
                Tok::Ident(s) if s == "all" => true,
                Tok::Ident(s) if s == "any" => false,
                t => return self.fail(format!("`at` takes `all` or `any`, not {}", describe(&t))),
            };
            let range = if self.keyword("cells") {
                self.bump();
                AtRange::Cells
            } else {
                let Tok::Ident(p) = self.bump() else {
                    return self.fail("`at all`/`at any` takes a parameter bound or `cells`");
                };
                if is_reserved(&p) {
                    return self.fail(format!("`{p}` is reserved, not a parameter"));
                }
                let Tok::Cmp(op) = self.bump() else {
                    return self.fail("expected a comparison after the parameter");
                };
                AtRange::Bound(p, op, self.snumber()?)
            };
            return Ok(Expr::At(Box::new(e), all, range));
        }
        Ok(e)
    }

    fn snumber(&mut self) -> Result<f64, ParseError> {
        let neg = if matches!(self.peek(), Tok::Minus) {
            self.bump();
            true
        } else {
            false
        };
        match self.bump() {
            Tok::Num(v) => Ok(if neg { -v } else { v }),
            t => self.fail(format!("expected a number, found {}", describe(&t))),
        }
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.and()?;
        while self.keyword("or") {
            self.bump();
            e = Expr::Or(Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.not()?;
        while self.keyword("and") {
            self.bump();
            e = Expr::And(Box::new(e), Box::new(self.not()?));
        }
        Ok(e)
    }

    fn not(&mut self) -> Result<Expr, ParseError> {
        if self.keyword("not") {
            self.bump();
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, ParseError> {
        let l = self.sum()?;
        if let Tok::Cmp(op) = *self.peek() {
            self.bump();
            let r = self.sum()?;
            if matches!(self.peek(), Tok::Cmp(_)) {
                return self.fail("comparisons do not chain; use `and`");
            }
            return Ok(Expr::Cmp(op, Box::new(l), Box::new(r)));
        }
        Ok(l)
    }

    fn sum(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.term()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => ArithOp::Add,
                Tok::Minus => ArithOp::Sub,
                _ => return Ok(e),
            };
            self.bump();
            e = Expr::Arith(op, Box::new(e), Box::new(self.term()?));
        }
    }

    fn term(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => ArithOp::Mul,
                Tok::Slash => ArithOp::Div,
                _ => return Ok(e),
            };
            self.bump();
            e = Expr::Arith(op, Box::new(e), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Tok::Minus) {
            self.bump();
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Tok::Num(v) => Ok(Expr::Num(v)),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.bump();
                    if BUILTINS.contains(&name.as_str()) {
                        let args = self.list(Self::barg)?;
                        return Ok(Expr::Call(name, args));
                    }
                    if is_reserved(&name) {
                        return self.fail(format!("`{name}` is reserved and takes no arguments"));
                    }
                    let sels = self.list(Self::selector)?;
                    return Ok(Expr::Select(name, sels));
                }
                match name.as_str() {
                    "replicates" => Ok(Expr::Counter(Counter::Replicates)),
                    "providers_reported" => Ok(Expr::Counter(Counter::ProvidersReported)),
                    w if is_reserved(w) => self.fail(format!("`{w}` cannot stand here")),
                    _ => Ok(Expr::Quantity(name)),
                }
            }
            t => self.fail(format!("expected a value, found {}", describe(&t))),
        }
    }

    fn list<T>(
        &mut self,
        item: fn(&mut Self) -> Result<T, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        let mut out = vec![item(self)?];
        while matches!(self.peek(), Tok::Comma) {
            self.bump();
            out.push(item(self)?);
        }
        self.expect(&Tok::RParen, "`)` or `,`")?;
        Ok(out)
    }

    /// barg := "ci" "=" number | "control" | "treatment" | qname | expr
    fn barg(&mut self) -> Result<Arg, ParseError> {
        if self.keyword("ci") && matches!(self.peek2(), Tok::Assign) {
            self.bump();
            self.bump();
            return match self.bump() {
                Tok::Num(v) => Ok(Arg::Ci(v)),
                t => self.fail(format!("`ci =` takes a number, not {}", describe(&t))),
            };
        }
        let is_arm_word = |s: &str| s == "control" || s == "treatment";
        if let Tok::Ident(s) = self.peek().clone()
            && is_arm_word(&s)
            && matches!(self.peek2(), Tok::Comma | Tok::RParen)
        {
            self.bump();
            return Ok(if s == "control" {
                Arg::Control
            } else {
                Arg::Treatment
            });
        }
        Ok(Arg::Expr(self.expr()?))
    }

    /// selector := "control" | "treatment" | pname "=" value | evalue
    fn selector(&mut self) -> Result<Selector, ParseError> {
        let Tok::Ident(name) = self.bump() else {
            return self
                .fail("a selector is `control`, `treatment`, `pname = value` or an enum value");
        };
        match name.as_str() {
            "control" => return Ok(Selector::Control),
            "treatment" => return Ok(Selector::Treatment),
            _ => {}
        }
        if matches!(self.peek(), Tok::Assign) {
            self.bump();
            let value = match self.peek().clone() {
                Tok::Ident(v) if v == "true" => {
                    self.bump();
                    Lit::Bool(true)
                }
                Tok::Ident(v) if v == "false" => {
                    self.bump();
                    Lit::Bool(false)
                }
                Tok::Ident(v) => {
                    self.bump();
                    Lit::Ident(v)
                }
                _ => Lit::Num(self.snumber()?),
            };
            return Ok(Selector::Fix(name, value));
        }
        if is_reserved(&name) {
            return self.fail(format!("`{name}` is reserved, not an enum value"));
        }
        Ok(Selector::Value(name))
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(v) => format!("`{v}`"),
        Tok::Ident(s) => format!("`{s}`"),
        Tok::LParen => "`(`".into(),
        Tok::RParen => "`)`".into(),
        Tok::Comma => "`,`".into(),
        Tok::Assign => "`=`".into(),
        Tok::Cmp(op) => format!("`{}`", op.as_str()),
        Tok::Plus => "`+`".into(),
        Tok::Minus => "`-`".into(),
        Tok::Star => "`*`".into(),
        Tok::Slash => "`/`".into(),
        Tok::End => "the end".into(),
    }
}

/// Parse a predicate or a guard.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
    };
    let e = p.expr()?;
    if !matches!(p.peek(), Tok::End) {
        return p.fail(format!(
            "unexpected {} after the predicate",
            describe(p.peek())
        ));
    }
    Ok(e)
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(s) => f.write_str(s),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Num(n) => write!(f, "{n}"),
        }
    }
}

impl fmt::Display for Expr {
    /// A fully parenthesised rendering: the golden form of a parse tree.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{n}"),
            Self::Counter(Counter::Replicates) => f.write_str("replicates"),
            Self::Counter(Counter::ProvidersReported) => f.write_str("providers_reported"),
            Self::Quantity(q) => f.write_str(q),
            Self::Select(q, sels) => {
                let s: Vec<String> = sels
                    .iter()
                    .map(|s| match s {
                        Selector::Control => "control".into(),
                        Selector::Treatment => "treatment".into(),
                        Selector::Fix(p, v) => format!("{p} = {v}"),
                        Selector::Value(v) => v.clone(),
                    })
                    .collect();
                write!(f, "{q}[{}]", s.join(", "))
            }
            Self::Call(name, args) => {
                let s: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Ci(c) => format!("ci = {c}"),
                        Arg::Control => "control".into(),
                        Arg::Treatment => "treatment".into(),
                        Arg::Expr(e) => e.to_string(),
                    })
                    .collect();
                write!(f, "{name}({})", s.join(", "))
            }
            Self::Neg(e) => write!(f, "(-{e})"),
            Self::Arith(op, l, r) => write!(f, "({l} {} {r})", op.as_str()),
            Self::Cmp(op, l, r) => write!(f, "({l} {} {r})", op.as_str()),
            Self::And(l, r) => write!(f, "({l} and {r})"),
            Self::Or(l, r) => write!(f, "({l} or {r})"),
            Self::Not(e) => write!(f, "(not {e})"),
            Self::At(e, all, range) => {
                let q = if *all { "all" } else { "any" };
                match range {
                    AtRange::Cells => write!(f, "({e} at {q} cells)"),
                    AtRange::Bound(p, op, v) => write!(f, "({e} at {q} {p} {} {v})", op.as_str()),
                }
            }
        }
    }
}
