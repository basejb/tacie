//! JSON with the same bytes as Python `json.dumps(v, ensure_ascii=False[, indent=n])`. The index files (chunks.json,
//! meta.json) and repos.json keep their format, so they are written in exactly this shape (the files can be compared
//! with `cmp`).
//!
//! - Without indent: `", "` between items and `": "` after a key. With indent n: n×depth spaces per line, `","` at the
//!   end of an item and `": "` after a key. Empty `[]`·`{}` stay on one line even with indent.
//! - Strings write `\"` `\\` `\n` `\r` `\t` `\b` `\f`, other characters below 0x20 as `\u00xx` (lowercase), and the
//!   rest as is (0x7f included).
//! - Integers as serde_json writes them, floats as Python `float.__repr__` does.

use std::fmt::Write as _;

use serde_json::{Map, Number, Value};

pub fn to_string(v: &Value, indent: Option<usize>) -> String {
    let mut w = Writer {
        out: String::new(),
        indent,
    };
    w.value(v, 0);
    w.out
}

/// 객체 목록을 `Value` 하나로 옮기지 않고 써요(chunks.json).
pub fn objects_to_string(items: &[Map<String, Value>], indent: Option<usize>) -> String {
    let mut w = Writer {
        out: String::new(),
        indent,
    };
    w.seq(items, 0, |w, m, depth| w.object(m, depth));
    w.out
}

struct Writer {
    out: String,
    indent: Option<usize>,
}

impl Writer {
    fn value(&mut self, v: &Value, depth: usize) {
        match v {
            Value::Null => self.out.push_str("null"),
            Value::Bool(true) => self.out.push_str("true"),
            Value::Bool(false) => self.out.push_str("false"),
            Value::Number(n) => number(&mut self.out, n),
            Value::String(s) => string(&mut self.out, s),
            Value::Array(a) => self.seq(a, depth, |w, x, depth| w.value(x, depth)),
            Value::Object(m) => self.object(m, depth),
        }
    }

    fn object(&mut self, m: &Map<String, Value>, depth: usize) {
        if m.is_empty() {
            self.out.push_str("{}");
            return;
        }
        self.out.push('{');
        for (i, (k, v)) in m.iter().enumerate() {
            self.between(i, depth + 1);
            string(&mut self.out, k);
            self.out.push_str(": ");
            self.value(v, depth + 1);
        }
        self.newline(depth);
        self.out.push('}');
    }

    fn seq<T>(&mut self, items: &[T], depth: usize, item: impl Fn(&mut Self, &T, usize)) {
        if items.is_empty() {
            self.out.push_str("[]");
            return;
        }
        self.out.push('[');
        for (i, x) in items.iter().enumerate() {
            self.between(i, depth + 1);
            item(self, x, depth + 1);
        }
        self.newline(depth);
        self.out.push(']');
    }

    /// 항목 앞이에요: 첫 항목이 아니면 구분자, 들여쓰기가 있으면 새 줄과 칸이에요.
    fn between(&mut self, i: usize, depth: usize) {
        if i > 0 {
            self.out
                .push_str(if self.indent.is_some() { "," } else { ", " });
        }
        self.newline(depth);
    }

    fn newline(&mut self, depth: usize) {
        if let Some(n) = self.indent {
            self.out.push('\n');
            self.out.extend(std::iter::repeat_n(' ', n * depth));
        }
    }
}

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn number(out: &mut String, n: &Number) {
    match n.as_f64() {
        Some(f) if n.is_f64() => float(out, f),
        _ => {
            let _ = write!(out, "{n}");
        }
    }
}

/// Python `float.__repr__`예요: 되돌려 읽으면 같은 값이 되는 가장 짧은 자리(Rust `{:e}`와 같아요)를,
/// 소수점 자리 decpt(값 = 0.자리 × 10^decpt)가 -4 < decpt ≤ 16이면 고정 소수점(`.0`을 붙여요)으로,
/// 아니면 `d.ddde±XX`(지수는 두 자리 이상)로 써요. serde_json 값에는 NaN·무한대가 없어요.
fn float(out: &mut String, f: f64) {
    let sci = format!("{f:e}"); // "-1.2345e-5", "1e16", "0e0"
    let (mantissa, exp) = sci.split_once('e').expect("{:e} always has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits = mantissa.replace('.', "");
    let decpt = exp + 1;
    out.push_str(sign);
    if -4 < decpt && decpt <= 16 {
        if decpt <= 0 {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', decpt.unsigned_abs() as usize));
            out.push_str(&digits);
        } else {
            let at = decpt as usize;
            if at >= digits.len() {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', at - digits.len()));
                out.push_str(".0");
            } else {
                out.push_str(&digits[..at]);
                out.push('.');
                out.push_str(&digits[at..]);
            }
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(
            out,
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.unsigned_abs()
        );
    }
}
