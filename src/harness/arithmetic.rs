//! Deterministic arithmetic over retrieved evidence — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/arithmetic.py`.
//!
//! Some questions ask for a number no single source states — the combined
//! population of three counties, how many listed films won an award, the days
//! between two dates. Every input is in the evidence by then and only the
//! arithmetic is missing, which an LLM does by writing digits one at a time and
//! gets wrong often enough to matter. So the model writes ONE expression and
//! this module evaluates it — parsed against a strict whitelist BEFORE
//! evaluation, with no builtins at all.

use serde_json::{Value, json};

use crate::harness::HarnessChat;

/// The whole expression; every figure is inline, none is long.
pub const COMPUTE_MAX_CHARS: usize = 400;

// ---------------------------------------------------------------------------
// Helper functions exposed to expressions
// ---------------------------------------------------------------------------

/// `_letters`: count the alphabetic characters across the given names.
/// Diacritics count; spaces, hyphens, apostrophes, digits and punctuation do not.
pub fn letters(values: &[Value]) -> Result<i64, String> {
    let mut total = 0i64;
    let mut seen: Vec<&Value> = Vec::new();
    fn flatten<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    flatten(item, out);
                }
            }
            other => out.push(other),
        }
    }
    for value in values {
        flatten(value, &mut seen);
    }
    for item in seen {
        match item {
            Value::String(text) => {
                total += text.chars().filter(|ch| ch.is_alphabetic()).count() as i64
            }
            other => {
                return Err(format!(
                    "letters() takes names, not {}",
                    json_type_name(other)
                ));
            }
        }
    }
    Ok(total)
}

/// `_digit_sum`: add up the decimal digits inside the given values (each digit
/// separately; ASCII digits only).
pub fn digit_sum(values: &[Value]) -> Result<i64, String> {
    let mut total = 0i64;
    let mut seen: Vec<&Value> = Vec::new();
    fn flatten<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    flatten(item, out);
                }
            }
            other => out.push(other),
        }
    }
    for value in values {
        flatten(value, &mut seen);
    }
    for item in seen {
        let text = match item {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            other => {
                return Err(format!(
                    "digit_sum() takes text or whole numbers, not {}",
                    json_type_name(other)
                ));
            }
        };
        total += text
            .chars()
            .filter_map(|ch| ch.to_digit(10))
            .map(|digit| digit as i64)
            .sum::<i64>();
    }
    Ok(total)
}

/// `_date_diff`: days between two ISO dates (inclusive of the earlier,
/// exclusive of the later).
pub fn date_diff(dates: &[String]) -> Result<i64, String> {
    if dates.len() != 2 {
        return Err("date_diff() takes exactly two ISO dates".to_string());
    }
    let mut parsed: Vec<chrono::NaiveDate> = Vec::new();
    for raw in dates {
        let text = raw.trim();
        let parts: Vec<&str> = text.split('-').collect();
        if parts.len() != 3 {
            return Err(format!("not an ISO date: {text:?}"));
        }
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| format!("not a valid ISO date: {text:?}"))?;
        let month: u32 = parts[1]
            .parse()
            .map_err(|_| format!("not a valid ISO date: {text:?}"))?;
        let day: u32 = parts[2]
            .parse()
            .map_err(|_| format!("not a valid ISO date: {text:?}"))?;
        let date = chrono::NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| format!("not a valid ISO date: {text:?}"))?;
        parsed.push(date);
    }
    Ok((parsed[1] - parsed[0]).num_days().abs())
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

// ---------------------------------------------------------------------------
// Expression model
// ---------------------------------------------------------------------------

const FUNCTIONS: [&str; 12] = [
    "abs",
    "round",
    "min",
    "max",
    "sum",
    "len",
    "int",
    "float",
    "sorted",
    "letters",
    "digit_sum",
    "date_diff",
];
/// Functions whose result is a number whatever they are handed.
const ALWAYS_NUMERIC: [&str; 8] = [
    "abs",
    "round",
    "int",
    "float",
    "len",
    "letters",
    "digit_sum",
    "date_diff",
];

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    List(Vec<Expr>),
    Name(String),
    Call(String, Vec<Expr>),
    Unary(char, Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Compare(Vec<(String, Expr)>),
    BoolOp(bool, Box<Expr>, Box<Expr>),
    IfElse(Box<Expr>, Box<Expr>, Box<Expr>),
}

fn is_numeric(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) => true,
        Expr::Unary(_, operand) => is_numeric(operand),
        Expr::Bin(_, left, right) => is_numeric(left) && is_numeric(right),
        Expr::IfElse(body, _, orelse) => is_numeric(body) && is_numeric(orelse),
        Expr::Compare(_) => true,
        Expr::Call(name, args) => {
            if ALWAYS_NUMERIC.contains(&name.as_str()) {
                return true;
            }
            if name == "sum" || name == "min" || name == "max" {
                return args
                    .iter()
                    .all(|arg| is_numeric(arg) || is_numeric_sequence(arg));
            }
            false
        }
        _ => false,
    }
}

fn is_numeric_sequence(expr: &Expr) -> bool {
    match expr {
        Expr::List(items) => items.iter().all(is_numeric),
        _ => false,
    }
}

/// `_check_expression`: reject anything outside the arithmetic whitelist.
fn check_expression(expr: &Expr) -> Result<(), String> {
    match expr {
        Expr::Name(name) => {
            if !FUNCTIONS.contains(&name.as_str()) {
                return Err(format!("unknown name '{name}'"));
            }
            Ok(())
        }
        Expr::Call(name, args) => {
            if !FUNCTIONS.contains(&name.as_str()) {
                return Err("only the listed functions may be called".to_string());
            }
            if name == "len"
                && let Some(Expr::Str(_)) = args.first()
            {
                return Err("len() on a string literal is ambiguous; use letters()".to_string());
            }
            for arg in args {
                if let Expr::Str(text) = arg
                    && text.chars().count() > 256
                {
                    return Err("string literal is too long".to_string());
                }
                check_expression(arg)?;
            }
            Ok(())
        }
        Expr::Bin('*', left, right) => {
            if !(is_numeric(left) && is_numeric(right)) {
                return Err("multiplication is only allowed on numbers".to_string());
            }
            check_expression(left)?;
            check_expression(right)
        }
        Expr::Bin('^', left, right) => {
            if !(is_numeric(left) && is_numeric(right)) {
                return Err("exponentiation is only allowed on numbers".to_string());
            }
            match right.as_ref() {
                Expr::Int(value) if value.abs() > 64 => {
                    return Err("exponent is too large".to_string());
                }
                Expr::Float(value) if value.abs() > 64.0 => {
                    return Err("exponent is too large".to_string());
                }
                _ => {}
            }
            check_expression(left)?;
            check_expression(right)
        }
        Expr::Unary(_, operand) => check_expression(operand),
        Expr::Bin(_, left, right) => {
            check_expression(left)?;
            check_expression(right)
        }
        Expr::Compare(chain) => {
            for (_, operand) in chain {
                check_expression(operand)?;
            }
            Ok(())
        }
        Expr::BoolOp(_, left, right) => {
            check_expression(left)?;
            check_expression(right)
        }
        Expr::IfElse(body, condition, orelse) => {
            check_expression(body)?;
            check_expression(condition)?;
            check_expression(orelse)
        }
        Expr::List(items) => {
            for item in items {
                check_expression(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    position: usize,
}

impl Parser {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.position += 1;
        }
        ch
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.position += 1;
        }
    }

    fn eat(&mut self, text: &str) -> bool {
        self.skip_ws();
        let chars: Vec<char> = text.chars().collect();
        if self.chars.len() >= self.position + chars.len()
            && self.chars[self.position..self.position + chars.len()] == chars[..]
        {
            // Do not treat a longer operator as a prefix match incorrectly.
            if text == "*" && self.peek_at(1) == Some('*') {
                return false;
            }
            if text == "/" && self.peek_at(1) == Some('/') {
                return false;
            }
            self.position += chars.len();
            return true;
        }
        false
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.position + offset).copied()
    }

    fn parse(mut self) -> Result<Expr, String> {
        let expr = self.parse_ternary()?;
        self.skip_ws();
        if self.position < self.chars.len() {
            return Err(format!("invalid syntax at position {}", self.position));
        }
        Ok(expr)
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let body = self.parse_or()?;
        if self.eat("if") {
            let condition = self.parse_or()?;
            if !self.eat("else") {
                return Err("expected 'else'".to_string());
            }
            let orelse = self.parse_ternary()?;
            return Ok(Expr::IfElse(
                Box::new(body),
                Box::new(condition),
                Box::new(orelse),
            ));
        }
        Ok(body)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.eat("or") {
            let right = self.parse_and()?;
            left = Expr::BoolOp(false, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_not()?;
        while self.eat("and") {
            let right = self.parse_not()?;
            left = Expr::BoolOp(true, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.eat("not") {
            let operand = self.parse_not()?;
            return Ok(Expr::Unary('!', Box::new(operand)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let first = self.parse_additive()?;
        let mut chain: Vec<(String, Expr)> = Vec::new();
        loop {
            self.skip_ws();
            let op = if self.eat("==") {
                "=="
            } else if self.eat("!=") {
                "!="
            } else if self.eat("<=") {
                "<="
            } else if self.eat(">=") {
                ">="
            } else if self.peek() == Some('<') {
                self.position += 1;
                "<"
            } else if self.peek() == Some('>') {
                self.position += 1;
                ">"
            } else {
                break;
            };
            let operand = self.parse_additive()?;
            chain.push((op.to_string(), operand));
        }
        if chain.is_empty() {
            Ok(first)
        } else {
            // chain[0] carries the left operand with an empty operator, then
            // each (op, right) pair; evaluation walks the sequence.
            let mut full: Vec<(String, Expr)> = Vec::with_capacity(chain.len() + 1);
            full.push((String::new(), first));
            full.extend(chain);
            Ok(Expr::Compare(full))
        }
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            self.skip_ws();
            let op = match self.peek() {
                Some('+') => {
                    self.position += 1;
                    '+'
                }
                Some('-') => {
                    self.position += 1;
                    '-'
                }
                _ => break,
            };
            let right = self.parse_multiplicative()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = if self.eat("//") {
                'F'
            } else if self.peek() == Some('*') {
                if self.peek_at(1) == Some('*') {
                    break;
                }
                self.position += 1;
                '*'
            } else if self.peek() == Some('/') {
                self.position += 1;
                '/'
            } else if self.peek() == Some('%') {
                self.position += 1;
                '%'
            } else {
                break;
            };
            let right = self.parse_unary()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        match self.peek() {
            Some('+') => {
                self.position += 1;
                Ok(Expr::Unary('+', Box::new(self.parse_unary()?)))
            }
            Some('-') => {
                self.position += 1;
                Ok(Expr::Unary('-', Box::new(self.parse_unary()?)))
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        let base = self.parse_atom()?;
        self.skip_ws();
        if self.eat("**") {
            let exponent = self.parse_unary()?;
            return Ok(Expr::Bin('^', Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.position += 1;
                let first = self.parse_ternary()?;
                self.skip_ws();
                if self.peek() == Some(',') {
                    let mut items = vec![first];
                    while self.eat(",") {
                        self.skip_ws();
                        if self.peek() == Some(')') {
                            break;
                        }
                        items.push(self.parse_ternary()?);
                        self.skip_ws();
                    }
                    self.expect(')')?;
                    Ok(Expr::List(items))
                } else {
                    self.expect(')')?;
                    Ok(first)
                }
            }
            Some('[') => {
                self.position += 1;
                let mut items: Vec<Expr> = Vec::new();
                loop {
                    self.skip_ws();
                    if self.peek() == Some(']') {
                        break;
                    }
                    items.push(self.parse_ternary()?);
                    self.skip_ws();
                    if self.peek() == Some(',') {
                        self.position += 1;
                        continue;
                    }
                    break;
                }
                self.expect(']')?;
                Ok(Expr::List(items))
            }
            Some('{') => {
                self.position += 1;
                let mut items: Vec<Expr> = Vec::new();
                loop {
                    self.skip_ws();
                    if self.peek() == Some('}') {
                        break;
                    }
                    items.push(self.parse_ternary()?);
                    self.skip_ws();
                    if self.peek() == Some(',') {
                        self.position += 1;
                        continue;
                    }
                    break;
                }
                self.expect('}')?;
                Ok(Expr::List(items))
            }
            Some('"') | Some('\'') => {
                let quote = self.bump().unwrap();
                let mut text = String::new();
                loop {
                    match self.bump() {
                        None => return Err("unterminated string literal".to_string()),
                        Some(ch) if ch == quote => break,
                        Some('\\') => {
                            if let Some(escaped) = self.bump() {
                                text.push(escaped);
                            }
                        }
                        Some(ch) => text.push(ch),
                    }
                }
                Ok(Expr::Str(text))
            }
            Some(ch)
                if ch.is_ascii_digit()
                    || (ch == '.' && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit())) =>
            {
                let mut text = String::new();
                let mut has_dot = false;
                let mut has_exp = false;
                loop {
                    match self.peek() {
                        Some(d) if d.is_ascii_digit() => {
                            text.push(d);
                            self.position += 1;
                        }
                        Some('.') if !has_dot && !has_exp => {
                            has_dot = true;
                            text.push('.');
                            self.position += 1;
                        }
                        Some(e) if (e == 'e' || e == 'E') && !has_exp => {
                            has_exp = true;
                            text.push(e);
                            self.position += 1;
                            if matches!(self.peek(), Some(sign) if sign == '+' || sign == '-') {
                                text.push(self.bump().unwrap());
                            }
                        }
                        _ => break,
                    }
                }
                if has_dot || has_exp {
                    text.parse::<f64>()
                        .map(Expr::Float)
                        .map_err(|_| format!("invalid number {text:?}"))
                } else {
                    text.parse::<i64>()
                        .map(Expr::Int)
                        .map_err(|_| format!("invalid number {text:?}"))
                }
            }
            Some(ch) if ch.is_alphabetic() || ch == '_' => {
                let mut name = String::new();
                while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
                    name.push(self.bump().unwrap());
                }
                match name.as_str() {
                    "True" => return Ok(Expr::Bool(true)),
                    "False" => return Ok(Expr::Bool(false)),
                    "and" | "or" | "not" | "if" | "else" => {
                        return Err(format!("invalid syntax: unexpected keyword {name:?}"));
                    }
                    _ => {}
                }
                self.skip_ws();
                if self.peek() == Some('(') {
                    self.position += 1;
                    let mut args: Vec<Expr> = Vec::new();
                    loop {
                        self.skip_ws();
                        if self.peek() == Some(')') {
                            break;
                        }
                        args.push(self.parse_ternary()?);
                        self.skip_ws();
                        if self.peek() == Some(',') {
                            self.position += 1;
                            continue;
                        }
                        break;
                    }
                    self.expect(')')?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Name(name))
                }
            }
            other => Err(format!("invalid syntax near {other:?}")),
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(expected) {
            self.position += 1;
            Ok(())
        } else {
            Err(format!("expected {expected:?}"))
        }
    }
}

fn parse_expression(text: &str) -> Result<Expr, String> {
    match Parser::new(text).parse()? {
        expr => Ok(expr),
    }
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Value_ {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    List(Vec<Value_>),
}

fn truthy(value: &Value_) -> bool {
    match value {
        Value_::Int(number) => *number != 0,
        Value_::Float(number) => *number != 0.0,
        Value_::Str(text) => !text.is_empty(),
        Value_::Bool(flag) => *flag,
        Value_::List(items) => !items.is_empty(),
    }
}

fn type_of(value: &Value_) -> &'static str {
    match value {
        Value_::Int(_) => "int",
        Value_::Float(_) => "float",
        Value_::Str(_) => "str",
        Value_::Bool(_) => "bool",
        Value_::List(_) => "list",
    }
}

fn to_f64(value: &Value_) -> Option<f64> {
    match value {
        Value_::Int(number) => Some(*number as f64),
        Value_::Float(number) => Some(*number),
        Value_::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn to_i64(value: &Value_) -> Option<i64> {
    match value {
        Value_::Int(number) => Some(*number),
        Value_::Float(number) => Some(*number as i64),
        Value_::Bool(flag) => Some(if *flag { 1 } else { 0 }),
        _ => None,
    }
}

fn numeric_pair(left: &Value_, right: &Value_) -> Option<(f64, f64)> {
    Some((to_f64(left)?, to_f64(right)?))
}

fn eval(expr: &Expr) -> Result<Value_, String> {
    match expr {
        Expr::Int(value) => Ok(Value_::Int(*value)),
        Expr::Float(value) => Ok(Value_::Float(*value)),
        Expr::Str(text) => Ok(Value_::Str(text.clone())),
        Expr::Bool(flag) => Ok(Value_::Bool(*flag)),
        Expr::List(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(eval(item)?);
            }
            Ok(Value_::List(out))
        }
        Expr::Name(name) => Err(format!("unknown name '{name}'")),
        Expr::Unary(op, operand) => {
            let value = eval(operand)?;
            match op {
                '-' => match value {
                    Value_::Int(number) => number
                        .checked_neg()
                        .map(Value_::Int)
                        .ok_or_else(|| "OverflowError: integer negation".to_string()),
                    Value_::Float(number) => Ok(Value_::Float(-number)),
                    other => Err(format!(
                        "TypeError: bad operand type for unary -: '{}'",
                        type_of(&other)
                    )),
                },
                '+' => match value {
                    Value_::Int(_) | Value_::Float(_) => Ok(value),
                    other => Err(format!(
                        "TypeError: bad operand type for unary +: '{}'",
                        type_of(&other)
                    )),
                },
                '!' => Ok(Value_::Bool(!truthy(&value))),
                _ => Err("TypeError: unsupported unary operator".to_string()),
            }
        }
        Expr::Bin(op, left, right) => {
            if *op == '&' {
                // Chained-comparison conjunction.
                let a = eval(left)?;
                if !truthy(&a) {
                    return Ok(Value_::Bool(false));
                }
                let b = eval(right)?;
                return Ok(Value_::Bool(truthy(&b)));
            }
            let left_value = eval(left)?;
            let right_value = eval(right)?;
            eval_bin(*op, left_value, right_value)
        }
        Expr::BoolOp(is_and, left, right) => {
            let left_value = eval(left)?;
            if *is_and {
                if !truthy(&left_value) {
                    return Ok(left_value);
                }
                return eval(right);
            }
            if truthy(&left_value) {
                return Ok(left_value);
            }
            eval(right)
        }
        Expr::Compare(chain) => {
            // chain[0] = ("", left); then (op, right) pairs; Python semantics
            // short-circuit and require every adjacent pair to hold.
            let mut left = eval(&chain[0].1)?;
            for (op, right_expr) in &chain[1..] {
                let right = eval(right_expr)?;
                if !compare_values(op, &left, &right)? {
                    return Ok(Value_::Bool(false));
                }
                left = right;
            }
            Ok(Value_::Bool(true))
        }
        Expr::IfElse(body, condition, orelse) => {
            if truthy(&eval(condition)?) {
                eval(body)
            } else {
                eval(orelse)
            }
        }
        Expr::Call(name, args) => {
            let mut values: Vec<Value_> = Vec::new();
            for arg in args {
                values.push(eval(arg)?);
            }
            eval_call(name, values)
        }
    }
}

fn eval_bin(op: char, left: Value_, right: Value_) -> Result<Value_, String> {
    match op {
        '+' => match (&left, &right) {
            (Value_::Str(a), Value_::Str(b)) => Ok(Value_::Str(format!("{a}{b}"))),
            (Value_::List(a), Value_::List(b)) => {
                let mut out = a.clone();
                out.extend(b.clone());
                Ok(Value_::List(out))
            }
            _ => {
                let (a, b) = numeric_pair(&left, &right).ok_or_else(|| {
                    format!(
                        "TypeError: unsupported operand type(s) for +: '{}' and '{}'",
                        type_of(&left),
                        type_of(&right)
                    )
                })?;
                if let (Value_::Int(x), Value_::Int(y)) = (&left, &right) {
                    return x
                        .checked_add(*y)
                        .map(Value_::Int)
                        .ok_or_else(|| "OverflowError: integer addition".to_string());
                }
                let _ = (a, b);
                Ok(Value_::Float(
                    to_f64(&left).unwrap() + to_f64(&right).unwrap(),
                ))
            }
        },
        '-' => {
            let (a, b) = numeric_pair(&left, &right).ok_or_else(|| {
                format!(
                    "TypeError: unsupported operand type(s) for -: '{}' and '{}'",
                    type_of(&left),
                    type_of(&right)
                )
            })?;
            if let (Value_::Int(x), Value_::Int(y)) = (&left, &right) {
                return x
                    .checked_sub(*y)
                    .map(Value_::Int)
                    .ok_or_else(|| "OverflowError: integer subtraction".to_string());
            }
            let _ = (a, b);
            Ok(Value_::Float(
                to_f64(&left).unwrap() - to_f64(&right).unwrap(),
            ))
        }
        '*' => match (&left, &right) {
            (Value_::Int(a), Value_::Int(b)) => a
                .checked_mul(*b)
                .map(Value_::Int)
                .ok_or_else(|| "OverflowError: integer multiplication".to_string()),
            (Value_::Str(text), Value_::Int(count)) => {
                Ok(Value_::Str(text.repeat((*count).max(0) as usize)))
            }
            _ => {
                let (a, b) = numeric_pair(&left, &right).ok_or_else(|| {
                    format!(
                        "TypeError: unsupported operand type(s) for *: '{}' and '{}'",
                        type_of(&left),
                        type_of(&right)
                    )
                })?;
                Ok(Value_::Float(a * b))
            }
        },
        '/' => {
            let (a, b) = numeric_pair(&left, &right)
                .ok_or_else(|| "TypeError: unsupported operand type(s) for /".to_string())?;
            if b == 0.0 {
                return Err("ZeroDivisionError: division by zero".to_string());
            }
            Ok(Value_::Float(a / b))
        }
        'F' => match (&left, &right) {
            (Value_::Int(a), Value_::Int(b)) => {
                if *b == 0 {
                    return Err("ZeroDivisionError: integer division by zero".to_string());
                }
                if *b == -1 {
                    return a
                        .checked_neg()
                        .map(Value_::Int)
                        .ok_or_else(|| "OverflowError: integer division overflow".to_string());
                }
                // Python floor division: adjust the truncating quotient when
                // the remainder and divisor have different signs.
                let quotient = a / b;
                let remainder = a % b;
                let quotient = if remainder != 0 && (remainder < 0) != (*b < 0) {
                    quotient - 1
                } else {
                    quotient
                };
                Ok(Value_::Int(quotient))
            }
            _ => {
                let (a, b) = numeric_pair(&left, &right)
                    .ok_or_else(|| "TypeError: unsupported operand type(s) for //".to_string())?;
                if b == 0.0 {
                    return Err("ZeroDivisionError: float floor division by zero".to_string());
                }
                Ok(Value_::Float((a / b).floor()))
            }
        },
        '%' => match (&left, &right) {
            (Value_::Int(a), Value_::Int(b)) => {
                if *b == 0 {
                    return Err("ZeroDivisionError: integer modulo by zero".to_string());
                }
                if *b == -1 {
                    return Ok(Value_::Int(0));
                }
                // Python modulo: the result carries the divisor's sign.
                let remainder = a % b;
                let remainder = if remainder != 0 && (remainder < 0) != (*b < 0) {
                    remainder + b
                } else {
                    remainder
                };
                Ok(Value_::Int(remainder))
            }
            _ => {
                let (a, b) = numeric_pair(&left, &right)
                    .ok_or_else(|| "TypeError: unsupported operand type(s) for %".to_string())?;
                if b == 0.0 {
                    return Err("ZeroDivisionError: float modulo".to_string());
                }
                let result = a - (a / b).floor() * b;
                Ok(Value_::Float(result))
            }
        },
        '^' => match (&left, &right) {
            (Value_::Int(base), Value_::Int(exponent)) if *exponent >= 0 => {
                if *exponent > 63 {
                    return Err("OverflowError: integer exponent overflow".to_string());
                }
                base.checked_pow(*exponent as u32)
                    .map(Value_::Int)
                    .ok_or_else(|| "OverflowError: integer exponent overflow".to_string())
            }
            _ => {
                let (a, b) = numeric_pair(&left, &right)
                    .ok_or_else(|| "TypeError: unsupported operand type(s) for **".to_string())?;
                Ok(Value_::Float(a.powf(b)))
            }
        },
        _ => Err("TypeError: unsupported binary operator".to_string()),
    }
}

fn compare_values(op: &str, left: &Value_, right: &Value_) -> Result<bool, String> {
    let ordering = match (left, right) {
        (Value_::Str(a), Value_::Str(b)) => a.partial_cmp(b),
        _ => {
            let a = to_f64(left);
            let b = to_f64(right);
            match (a, b) {
                (Some(a), Some(b)) => a.partial_cmp(&b),
                _ => {
                    return Err(format!(
                        "TypeError: '{}' not supported between instances of '{}' and '{}'",
                        op,
                        type_of(left),
                        type_of(right)
                    ));
                }
            }
        }
    };
    let ordering = ordering.ok_or_else(|| "TypeError: comparison failed".to_string())?;
    Ok(match op {
        "==" => ordering == std::cmp::Ordering::Equal,
        "!=" => ordering != std::cmp::Ordering::Equal,
        "<" => ordering == std::cmp::Ordering::Less,
        "<=" => ordering != std::cmp::Ordering::Greater,
        ">" => ordering == std::cmp::Ordering::Greater,
        ">=" => ordering != std::cmp::Ordering::Less,
        _ => false,
    })
}

fn eval_call(name: &str, values: Vec<Value_>) -> Result<Value_, String> {
    // Convert helper values back into serde_json values for the shared helpers.
    let to_json = |value: &Value_| -> Value {
        match value {
            Value_::Int(number) => json!(number),
            Value_::Float(number) => json!(number),
            Value_::Str(text) => json!(text),
            Value_::Bool(flag) => json!(flag),
            Value_::List(items) => json!(items.iter().map(to_json_ref).collect::<Vec<Value>>()),
        }
    };
    fn to_json_ref(value: &Value_) -> Value {
        match value {
            Value_::Int(number) => json!(number),
            Value_::Float(number) => json!(number),
            Value_::Str(text) => json!(text),
            Value_::Bool(flag) => json!(flag),
            Value_::List(items) => Value::Array(items.iter().map(to_json_ref).collect()),
        }
    }
    match name {
        "abs" => {
            expect_args(name, &values, 1)?;
            match &values[0] {
                Value_::Int(number) => Ok(Value_::Int(number.abs())),
                Value_::Float(number) => Ok(Value_::Float(number.abs())),
                Value_::Bool(_) => Ok(Value_::Int(to_i64(&values[0]).unwrap().abs())),
                other => Err(format!(
                    "TypeError: bad operand type for abs(): '{}'",
                    type_of(other)
                )),
            }
        }
        "round" => {
            if values.is_empty() || values.len() > 2 {
                return Err("TypeError: round() takes 1 or 2 arguments".to_string());
            }
            let number = to_f64(&values[0]).ok_or_else(|| {
                format!(
                    "TypeError: type {} doesn't define __round__ method",
                    type_of(&values[0])
                )
            })?;
            if values.len() == 1 {
                return Ok(Value_::Int(number.round_ties_even() as i64));
            }
            let digits = to_i64(&values[1]).ok_or_else(|| {
                "TypeError: 'float' object cannot be interpreted as an integer".to_string()
            })?;
            let factor = 10f64.powi(digits as i32);
            Ok(Value_::Float((number * factor).round_ties_even() / factor))
        }
        "min" | "max" => {
            if values.is_empty() {
                return Err(format!(
                    "TypeError: {name} expected at least 1 argument, got 0"
                ));
            }
            let items: Vec<Value_> = if values.len() == 1 {
                match &values[0] {
                    Value_::List(items) => items.clone(),
                    other => vec![other.clone()],
                }
            } else {
                values.clone()
            };
            if items.is_empty() {
                return Err(format!("TypeError: {name}() arg is an empty sequence"));
            }
            let mut best = items[0].clone();
            for item in items.iter().skip(1) {
                let better = compare_values(if name == "min" { "<" } else { ">" }, item, &best)?;
                if better {
                    best = item.clone();
                }
            }
            Ok(best)
        }
        "sum" => {
            if values.is_empty() || values.len() > 2 {
                return Err("TypeError: sum() takes 1 or 2 arguments".to_string());
            }
            let items: Vec<Value_> = match &values[0] {
                Value_::List(items) => items.clone(),
                other => vec![other.clone()],
            };
            let mut total = match values.get(1) {
                Some(Value_::Int(start)) => Value_::Int(*start),
                Some(Value_::Float(start)) => Value_::Float(*start),
                Some(other) => {
                    return Err(format!(
                        "TypeError: unsupported operand type(s) for +: 'int' and '{}'",
                        type_of(other)
                    ));
                }
                None => Value_::Int(0),
            };
            for item in items {
                total = eval_bin('+', total, item)?;
            }
            Ok(total)
        }
        "len" => {
            expect_args(name, &values, 1)?;
            match &values[0] {
                Value_::Str(text) => Ok(Value_::Int(text.chars().count() as i64)),
                Value_::List(items) => Ok(Value_::Int(items.len() as i64)),
                other => Err(format!(
                    "TypeError: object of type '{}' has no len()",
                    type_of(other)
                )),
            }
        }
        "int" => {
            expect_args(name, &values, 1)?;
            match &values[0] {
                Value_::Str(text) => text.trim().parse::<i64>().map(Value_::Int).map_err(|_| {
                    format!("ValueError: invalid literal for int() with base 10: {text:?}")
                }),
                other => to_i64(other).map(Value_::Int).ok_or_else(|| {
                    format!(
                        "TypeError: int() argument must be a string or a number, not '{}'",
                        type_of(other)
                    )
                }),
            }
        }
        "float" => {
            expect_args(name, &values, 1)?;
            match &values[0] {
                Value_::Str(text) => text.trim().parse::<f64>().map(Value_::Float).map_err(|_| {
                    format!("ValueError: could not convert string to float: {text:?}")
                }),
                other => to_f64(other).map(Value_::Float).ok_or_else(|| {
                    format!(
                        "TypeError: float() argument must be a string or a number, not '{}'",
                        type_of(other)
                    )
                }),
            }
        }
        "sorted" => {
            expect_args(name, &values, 1)?;
            let mut items = match &values[0] {
                Value_::List(items) => items.clone(),
                other => vec![other.clone()],
            };
            // Insertion sort with the shared comparison (small inputs only).
            let mut result: Vec<Value_> = Vec::new();
            for item in items.drain(..) {
                let mut inserted = false;
                let mut index = 0usize;
                while index < result.len() {
                    if compare_values("<", &item, &result[index])? {
                        result.insert(index, item.clone());
                        inserted = true;
                        break;
                    }
                    index += 1;
                }
                if !inserted {
                    result.push(item);
                }
            }
            Ok(Value_::List(result))
        }
        "letters" => {
            let json_values: Vec<Value> = values.iter().map(to_json).collect();
            let count = letters(&json_values).map_err(|error| format!("TypeError: {error}"))?;
            Ok(Value_::Int(count))
        }
        "digit_sum" => {
            let json_values: Vec<Value> = values.iter().map(to_json).collect();
            let count = digit_sum(&json_values).map_err(|error| format!("TypeError: {error}"))?;
            Ok(Value_::Int(count))
        }
        "date_diff" => {
            let mut dates: Vec<String> = Vec::new();
            for value in &values {
                match value {
                    Value_::Str(text) => dates.push(text.clone()),
                    other => {
                        return Err(format!(
                            "TypeError: date_diff() takes ISO date strings, not {}",
                            type_of(other)
                        ));
                    }
                }
            }
            let days = date_diff(&dates).map_err(|error| format!("ValueError: {error}"))?;
            Ok(Value_::Int(days))
        }
        _ => Err("only the listed functions may be called".to_string()),
    }
}

fn expect_args(name: &str, values: &[Value_], count: usize) -> Result<(), String> {
    if values.len() != count {
        return Err(format!(
            "TypeError: {name}() takes exactly {count} argument(s) ({} given)",
            values.len()
        ));
    }
    Ok(())
}

/// `_format_number`: render a computed number without float noise.
fn format_number(value: &Value_) -> String {
    match value {
        Value_::Int(number) => number.to_string(),
        Value_::Float(number) => {
            if number.fract() == 0.0 && number.abs() < 1e15 {
                return (*number as i64).to_string();
            }
            let mut text = format!("{number:.6}");
            while text.ends_with('0') {
                text.pop();
            }
            if text.ends_with('.') {
                text.pop();
            }
            text
        }
        Value_::Bool(flag) => flag.to_string(),
        Value_::Str(text) => text.clone(),
        Value_::List(_) => "list".to_string(),
    }
}

/// `compute`: evaluate an LLM-written arithmetic expression. Returns
/// `(rendered, error)`; exactly one of the two is non-empty.
pub fn compute(expression: &str) -> (String, String) {
    let expression = expression.trim();
    if expression.is_empty() {
        return (String::new(), "empty expression".to_string());
    }
    if expression.chars().count() > COMPUTE_MAX_CHARS {
        return (
            String::new(),
            format!("expression is longer than {COMPUTE_MAX_CHARS} characters"),
        );
    }
    let expr = match parse_expression(expression) {
        Ok(expr) => expr,
        Err(error) => return (String::new(), format!("does not parse ({error})")),
    };
    if let Err(problem) = check_expression(&expr) {
        return (String::new(), problem);
    }
    let value = match eval(&expr) {
        Ok(value) => value,
        Err(error) => return (String::new(), format!("failed to evaluate ({error})")),
    };
    match value {
        Value_::Bool(_) => (String::new(), "result is bool, not a number".to_string()),
        Value_::Str(_) => (String::new(), "result is str, not a number".to_string()),
        Value_::List(_) => (String::new(), "result is list, not a number".to_string()),
        Value_::Float(number) if !number.is_finite() => {
            (String::new(), "result is not a finite number".to_string())
        }
        Value_::Int(_) => (format_number(&value), String::new()),
        Value_::Float(_) => (format_number(&value), String::new()),
    }
}

// ---------------------------------------------------------------------------
// compute_from_facts
// ---------------------------------------------------------------------------

/// `_COMPUTE_SYSTEM`.
pub const COMPUTE_SYSTEM: &str = r#"You are given the ORIGINAL question and every fact discovered so far. Decide
whether that question asks for a NUMBER that NO fact states outright but that FOLLOWS ARITHMETICALLY
from figures the facts DO state — a sum, a difference, a count, an average, a percentage, a unit
conversion, an elapsed span.

If it does, compute it by writing ONE Python expression with every figure substituted as a literal.
The expression is evaluated on its own: no variables, no assignments, no imports, no attributes, no
subscripts. The only functions available are abs, round, min, max, sum, len, int, float, sorted,
letters, digit_sum and date_diff.
  combined population of three  -> 12345 + 6789 + 101112
  how many of the listed items  -> len(["Alpha", "Beta", "Gamma"])
  what percentage one figure is -> 100 * 4523 / 18092
  years between two dates       -> 1998 - 1954
  days between two dates        -> date_diff("1941-07-28", "1959-07-17")
  letters in a set of names     -> letters("Ada Lovelace", "Alan Turing")
  digits of a postcode added up -> digit_sum("L7 7BN")

ADDING UP THE DIGITS of a postcode, a house number, a serial number, a year or an address: use
digit_sum(...), and never read the digits out by hand. It adds each digit separately, which is what
such a question means — digit_sum("L7 7BN") is 7+7 = 14, digit_sum("2020") is 2+0+2+0 = 4. Pass the
identifier EXACTLY as the facts write it, letters and spaces included; they are ignored. It is the
WRONG tool for whole numbers the facts state separately — two populations, two prices, two years are
added as plain literals (12345 + 6789), not fed to digit_sum.

COUNTING LETTERS: use letters(...), NEVER len(...) on a name. len counts spaces, hyphens and
apostrophes as though they were letters, so it is wrong by exactly the amount nobody notices
(len("Ada Lovelace") is 12; the name has 11 letters). letters(...) takes any number of names, or one
list of them, and counts alphabetic characters only. Spell each name EXACTLY as the facts give it,
including any middle name or accent — and if the facts do not show a name in full, that figure is
missing, so return "needed": false rather than counting a partial name.

DAYS BETWEEN TWO DATES: when the question asks "how many days after X did Y happen" / "how many days
between two dates", use date_diff("YYYY-MM-DD", "YYYY-MM-DD") with the two dates EXACTLY as the facts
write them. Do NOT subtract the years (1959 - 1941) — that is the wrong quantity for a days question
(18 is years, not days). If either date is not a full YYYY-MM-DD in the facts, the figure is missing,
so return "needed": false rather than approximating.

AGE (an age, or an age difference, at some event): the facts almost always give a birth YEAR and an
event YEAR; the age is `event_year - birth_year` (or `birth_year - event_year`, taken as the positive
difference). You do NOT need the birth month or day — the year is enough. If the facts give FULL dates
(YYYY-MM-DD), prefer date_diff(...) which handles the day correctly; otherwise subtract the years.
Example: "elected in 2010, born 1971" -> 2010 - 1971. If the event year is BEFORE the birth year, the
difference is `birth_year - event_year` (use abs(...)). Never refuse because the birthday is not a
full date — the YEAR is sufficient.

PERCENTAGE (what percent / what share / what proportion / what fraction): `100 * part / whole`, where
`part` and `whole` are the exact figures from the facts. Example:
  "2.7 million Tamazight speakers out of 556 million total" -> 100 * 2.7 / 556
Do not round to an integer unless the question asks for that; keep the source figures exact.

UNIT CONVERSION (a speed, rate, or span in mixed units): convert inside the expression. A speed in
km/h becomes m/s by dividing by 3.6. Example for a difference in m/s between a fish and a swimmer:
  fish_kmh / 3.6 - 50 / swimmer_seconds      -> e.g. 132 / 3.6 - 50 / 21.07
Use the EXACT figures the facts state (do not round 21.07 to 21); if the facts give the speed already
in m/s, use it directly without dividing.

MULTIPLICATION (a rate times a count, e.g. dollars per day times days): multiply the RATE by the
COUNT exactly as the facts state them. Read the rate's NUMBER from the facts. Example: "a suggested
donation of $25 per day, kept up for 49 days" -> 25 * 49. If the facts state the rate as 1 but the
question calls it "a suggested donation", still use the exact figure the facts give — never substitute
a made-up base amount.

Prefer computing over giving up: when the question asks for a derivable number and the facts
provide the figures (even if in different units or spread across several facts), WRITE the
expression and compute it. In particular, questions asking for an AGE DIFFERENCE, a PERCENTAGE,
a SPEED DIFFERENCE (with unit conversion), or a MULTIPLICATION (a rate times a count) are exactly
what this tool is for.

Return "needed": false, with an empty expression, ONLY when:
- the ORIGINAL question does not ask for a number;
- a fact already states that number outright — a value you would only be restating is not a
  calculation;
- a figure the calculation needs is genuinely absent from the facts, or a list the count depends on
  is not shown to be complete. NEVER invent, estimate, recall or infer a figure. When input is
  missing, say so and return "needed": false — a wrong number is worse than none — but first check
  that the figure really is absent (e.g. the age's birth YEAR is enough; you do not need the month).

"label" names what the number IS, as a short noun phrase ("combined population of the three
counties"), so a later step can use the result without re-deriving it.
"uses" lists the INDEX NUMBERS of the facts whose figures you substituted.
Output ONLY JSON, no prose, no code fences:
{"needed": true/false, "expression": "<one Python expression, or empty>", "label": "<short noun phrase>", "uses": [<index number>, ...]}"#;

/// `_render_facts`: one fact per line, prefixed with its index.
pub fn render_facts(facts: &[String]) -> String {
    facts
        .iter()
        .enumerate()
        .map(|(index, fact)| format!("[{index}] {fact}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `compute_from_facts`: ask the LLM whether `question` asks for a derivable
/// number and, if so, write + safely evaluate the expression over `facts`.
/// Returns `None` when no derivation is needed or possible.
pub async fn compute_from_facts(
    chat: &dyn HarnessChat,
    question: &str,
    facts: &[String],
    fit_budget: Option<usize>,
) -> Option<Value> {
    if question.is_empty() || facts.is_empty() {
        return None;
    }
    let user = format!(
        "Facts discovered so far:\n{}\n\nOriginal question:\n{}\n\nOutput JSON:",
        render_facts(facts),
        question
    );
    let budget = fit_budget.unwrap_or_else(|| chat.max_length());
    let (_, messages) =
        crate::harness::message_fit_in(crate::harness::form_message(COMPUTE_SYSTEM, &user), budget);
    let system = messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(COMPUTE_SYSTEM);
    let history = messages[1..].to_vec();
    let gen_conf = json!({"temperature": 0.0});
    let raw = chat.chat(system, &history, &gen_conf).await.ok()?;

    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(&raw, ""), "")
        .trim()
        .to_string();
    let data: Value = serde_json::from_str(&cleaned).ok()?;
    if !data.is_object()
        || !data
            .get("needed")
            .map(|value| value == &json!(true))
            .unwrap_or(false)
    {
        return None;
    }
    let expression = data
        .get("expression")
        .map(|value| match value {
            Value::String(text) => text.trim().to_string(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    if expression.is_empty() {
        return None;
    }
    let label = data
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Value calculated from the facts found")
        .to_string();
    let mut uses: Vec<i64> = Vec::new();
    if let Some(items) = data.get("uses").and_then(Value::as_array) {
        for item in items {
            if let Some(value) = item
                .as_i64()
                .or_else(|| item.as_f64().map(|float| float as i64))
                .or_else(|| {
                    item.as_str()
                        .and_then(|text| text.trim().parse::<i64>().ok())
                })
            {
                uses.push(value);
            }
        }
    }
    let (value, error) = compute(&expression);
    if !error.is_empty() {
        return None;
    }
    Some(json!({
        "needed": true,
        "label": label,
        "value": value,
        "expression": expression,
        "uses": uses,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_mirror_upstream() {
        assert_eq!(
            letters(&[json!("Ada Lovelace"), json!("Alan Turing")]).unwrap(),
            21
        );
        assert_eq!(letters(&[json!(["Ada", "Alan"])]).unwrap(), 7);
        assert!(letters(&[json!(42)]).is_err());
        assert_eq!(digit_sum(&[json!("L7 7BN")]).unwrap(), 14);
        assert_eq!(digit_sum(&[json!(2020)]).unwrap(), 4);
        assert_eq!(
            date_diff(&["1941-07-28".to_string(), "1959-07-17".to_string()]).unwrap(),
            6563
        );
        assert!(date_diff(&["not-a-date".to_string(), "1959-07-17".to_string()]).is_err());
    }

    #[test]
    fn compute_arithmetic_and_formatting() {
        assert_eq!(
            compute("12345 + 6789 + 101112"),
            ("120246".to_string(), String::new())
        );
        assert_eq!(
            compute("100 * 4523 / 18092"),
            ("25".to_string(), String::new())
        );
        assert_eq!(compute("0.1 + 0.2"), ("0.3".to_string(), String::new()));
        assert_eq!(compute("1998 - 1954"), ("44".to_string(), String::new()));
        assert_eq!(
            compute("100 * 2.7 / 556"),
            ("0.485612".to_string(), String::new())
        );
        assert_eq!(compute("2 ** 10"), ("1024".to_string(), String::new()));
        assert_eq!(compute("7 // 2"), ("3".to_string(), String::new()));
        assert_eq!(compute("7 % 3"), ("1".to_string(), String::new()));
        assert_eq!(compute("-7 % 3"), ("2".to_string(), String::new()));
    }

    #[test]
    fn compute_refuses_unsafe_or_invalid() {
        assert!(!compute("").1.is_empty());
        assert!(
            compute(&"1+".repeat(300))
                .1
                .starts_with("expression is longer")
        );
        assert!(
            compute("__import__('os')")
                .1
                .contains("only the listed functions may be called")
        );
        assert_eq!(
            compute("open('x')").1,
            "only the listed functions may be called"
        );
        assert!(
            compute("2 * 'abc'")
                .1
                .contains("multiplication is only allowed on numbers")
        );
        assert!(compute("2 ** 100").1.contains("exponent is too large"));
        assert!(compute("len(\"Ada Lovelace\")").1.contains("use letters()"));
        assert!(compute("letters(42)").1.contains("letters() takes names"));
        assert!(compute("1 / 0").1.contains("division by zero"));
        assert!(compute("True").1.contains("result is bool, not a number"));
        assert!(compute("abs(-5)").0 == "5");
        assert_eq!(compute("round(2.5)").0, "2");
        assert_eq!(
            compute("sorted([3, 1, 2])").1,
            "result is list, not a number"
        );
        assert_eq!(compute("sum([1, 2, 3]) + max(4, 2) + min([9, 5])").0, "15");
    }
}
