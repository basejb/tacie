//! Follows Python text rules (`str.strip`, `splitlines`, `html.unescape` and the like) exactly, down to the byte.
//! Chunk text is built only with these: if even one character differs, the first refresh after an engine change
//! embeds every chunk again and card line numbers change. `str::lines`, `trim`, byte slicing and `from_utf8_lossy`
//! (which inserts U+FFFD) differ from these rules, so they are not used for chunk text.

use std::borrow::Cow;
use std::path::Path;

use serde_json::Value;

/// Python `str.isspace()`예요: 유니코드 White_Space에 `\x1c`–`\x1f`(정보 구분 문자)를 더한 것이에요.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python `str.strip()`이에요(인자 없이).
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Python `str.splitlines()`의 줄 경계예요(`\r\n`은 하나로 봐요).
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Python `str.splitlines()`예요: 경계는 `\n` `\r` `\r\n` `\x0b` `\x0c` `\x1c` `\x1d` `\x1e` `\x85` U+2028 U+2029이고,
/// 끝의 경계 뒤에는 빈 항목이 없어요.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_line_break(c) {
            continue;
        }
        out.push(&s[start..i]);
        start = i + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Python `s[:n]`이에요: 앞의 코드 포인트 n개예요.
pub fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Python `bytes.decode("utf-8", errors="ignore")`예요: 올바른 UTF-8 부분만 이어요(BOM은 남아요).
pub fn decode_ignore(bytes: &[u8]) -> String {
    bytes.utf8_chunks().map(|chunk| chunk.valid()).collect()
}

/// 파일을 UTF-8로 읽고 올바르지 않은 바이트는 버려요. 줄 끝 `\r\n`·`\r`은 `\n`으로 바꾸지 않아요: 조각은 늘
/// `splitlines`로 나누고, 거기서는 둘이 같은 줄이 돼요.
pub fn read_text(path: &Path) -> std::io::Result<String> {
    std::fs::read(path).map(|bytes| decode_ignore(&bytes))
}

/// Python의 참·거짓(`if x`, `x or y`)이에요: null, false, 0, 0.0, "", [], {}가 거짓이에요.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_none_or(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// Python `html.unescape`예요(Python 3.12 html/__init__.py): `&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)`를
/// 왼쪽부터 바꿔요. 이름은 Python `html5` 표와 같은 WHATWG 표(`htmlize::ENTITIES`, 2,231개)에서 찾고, 통째로 없으면
/// 가장 긴 앞부분(두 글자 이상)을 바꾸고 나머지는 그대로 둬요(`&notit;` → `¬it;`). 숫자는 Python의
/// `_invalid_charrefs`(0, CR, Windows-1252 자리) → 서로게이트·0x10FFFF 넘음은 U+FFFD → `_invalid_codepoints`는 지움 →
/// 그 글자 순이에요. `htmlize::unescape`(HTML 파서 규칙)는 이 몇 곳에서 Python과 달라서 쓰지 않아요.
///
/// 4,300자리가 넘는 10진 숫자 참조는 U+FFFD예요(Python 3.12 `html.unescape`는 정수 글자 길이 제한으로 ValueError를
/// 던지는 곳이에요). 슬랙은 글의 `&`를 `&amp;`로 보내서 원본에 숫자 참조가 그대로 오지 않아요.
pub fn html_unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match charref(after) {
            Some((len, replacement)) => {
                out.push_str(&replacement);
                rest = &after[len..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// `&` 뒤의 참조 하나예요: (쓴 바이트 수, 바꿀 글)이에요. 맞는 모양이 아니면 None이에요.
fn charref(after: &str) -> Option<(usize, Cow<'static, str>)> {
    let semi = |end: usize| end + usize::from(after[end..].starts_with(';'));
    if let Some(body) = after.strip_prefix('#') {
        let dec = body.bytes().take_while(u8::is_ascii_digit).count();
        if dec > 0 {
            return Some((semi(1 + dec), numeric_ref(&body[..dec], 10)));
        }
        let hex = body.strip_prefix(['x', 'X'])?;
        let digits = hex.bytes().take_while(u8::is_ascii_hexdigit).count();
        if digits == 0 {
            return None;
        }
        return Some((semi(2 + digits), numeric_ref(&hex[..digits], 16)));
    }
    let mut end = 0;
    for (n, (i, c)) in after.char_indices().enumerate() {
        if n == 32 || matches!(c, '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';') {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 {
        return None;
    }
    let name = &after[..semi(end)];
    if let Some(v) = entity(name) {
        return Some((name.len(), Cow::Borrowed(v)));
    }
    // Python `for x in range(len(s)-1, 1, -1)`: 가장 긴 앞부분부터, 두 글자까지예요
    let starts: Vec<usize> = name.char_indices().map(|(i, _)| i).collect();
    for &cut in starts.iter().skip(2).rev() {
        if let Some(v) = entity(&name[..cut]) {
            return Some((name.len(), Cow::Owned(format!("{v}{}", &name[cut..]))));
        }
    }
    Some((name.len(), Cow::Owned(format!("&{name}"))))
}

/// 이름 붙은 참조 하나예요(`;`가 붙은 이름과 붙지 않은 옛 이름이 따로 있어요).
fn entity(name: &str) -> Option<&'static str> {
    let key = format!("&{name}");
    htmlize::ENTITIES
        .get(key.as_bytes())
        .and_then(|v| std::str::from_utf8(v).ok())
}

/// 숫자 참조의 글이에요(Python `_replace_charref`의 숫자 쪽).
fn numeric_ref(digits: &str, radix: u32) -> Cow<'static, str> {
    // 0x10FFFF를 넘으면 더 셀 필요가 없어요(어차피 U+FFFD예요)
    let num = digits.chars().fold(0u32, |n, d| {
        n.saturating_mul(radix)
            .saturating_add(d.to_digit(radix).unwrap_or(0))
            .min(0x11_0000)
    });
    if let Some(c) = invalid_charref(num) {
        return Cow::Borrowed(c);
    }
    if (0xD800..=0xDFFF).contains(&num) || num > 0x10_FFFF {
        return Cow::Borrowed("\u{fffd}");
    }
    if invalid_codepoint(num) {
        return Cow::Borrowed("");
    }
    char::from_u32(num).map_or(Cow::Borrowed("\u{fffd}"), |c| Cow::Owned(c.to_string()))
}

/// Python `_invalid_charrefs`예요: 0, CR, 그리고 0x80–0x9F의 Windows-1252 글자예요.
fn invalid_charref(num: u32) -> Option<&'static str> {
    const CP1252: [&str; 32] = [
        "\u{20ac}", "\u{81}", "\u{201a}", "\u{192}", "\u{201e}", "\u{2026}", "\u{2020}",
        "\u{2021}", "\u{2c6}", "\u{2030}", "\u{160}", "\u{2039}", "\u{152}", "\u{8d}", "\u{17d}",
        "\u{8f}", "\u{90}", "\u{2018}", "\u{2019}", "\u{201c}", "\u{201d}", "\u{2022}", "\u{2013}",
        "\u{2014}", "\u{2dc}", "\u{2122}", "\u{161}", "\u{203a}", "\u{153}", "\u{9d}", "\u{17e}",
        "\u{178}",
    ];
    match num {
        0x00 => Some("\u{fffd}"),
        0x0d => Some("\r"),
        0x80..=0x9f => Some(CP1252[(num - 0x80) as usize]),
        _ => None,
    }
}

/// Python `_invalid_codepoints`예요: 제어 문자 일부와 유니코드 비문자예요.
fn invalid_codepoint(num: u32) -> bool {
    matches!(num, 0x1..=0x8 | 0xb | 0xe..=0x1f | 0x7f..=0x9f | 0xfdd0..=0xfdef)
        || (num & 0xfffe == 0xfffe && num <= 0x10_ffff)
}
