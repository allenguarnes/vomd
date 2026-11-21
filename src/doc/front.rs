//! YAML front matter shown as GitHub shows it: a table with a row per top-level key, its values
//! typed the way Ruby's Psych types them (what GitHub reads front matter with) and printed the
//! way Ruby prints them. Nested values go inside their row's cell as an outline (VOMD's table
//! cells hold text, not tables). Front matter that isn't valid YAML is shown as it is written,
//! under GitHub's error, and front matter that isn't a map isn't front matter at all.
//!
//! The table is made of the events a Markdown table makes, so the builder, layout, selection,
//! copy and find treat it as any table.

use std::collections::HashMap;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, CowStr, Event, Tag, TagEnd};

use super::yaml::{self, Node};

/// Front matter larger than this isn't read: it is shown as an error and its source.
const MAX_BYTES: usize = 1 << 20;
/// Integers in base 2, 8 or 16 longer than this many digits are shown as written (converting
/// them to decimal takes time that grows with the square of their length).
const MAX_RADIX_DIGITS: usize = 1024;

/// The events that stand for front matter `src` (from its opening `---` line to its closing one,
/// a BOM included): the table, or the error and the source. `None` when it isn't a map (empty,
/// a scalar or a list), which GitHub shows as Markdown.
pub(super) fn events(src: &str) -> Option<Vec<Event<'static>>> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    // The lines between the opening line and the closing one.
    let last = src
        .strip_suffix('\n')
        .unwrap_or(src)
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let first = src.find('\n').map_or(src.len(), |i| i + 1).min(last);
    // A lone CR is a line break to YAML too.
    let yaml = lf(&src[first..last], '\n');
    let error = if src.len() > MAX_BYTES {
        "front matter over 1 MiB is not read".to_owned()
    } else {
        match yaml::read(&yaml) {
            Err(e) => e,
            Ok(Some((_, true))) => "Alias parsing is not enabled.".to_owned(),
            Ok(Some((node, false))) => {
                return match value(&node) {
                    Value::Map(map) => Some(table(&map)),
                    _ => None,
                };
            }
            Ok(None) => return None,
        }
    };
    let mut code = lf(src, '\r').into_owned();
    if !code.ends_with('\n') {
        code.push('\n');
    }
    let caution = Some(BlockQuoteKind::Caution);
    Some(vec![
        Event::Start(Tag::BlockQuote(caution)),
        Event::Start(Tag::Paragraph),
        Event::Text(format!("Error in user YAML: {error}").into()),
        Event::End(TagEnd::Paragraph),
        Event::End(TagEnd::BlockQuote(caution)),
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced("yaml".into()))),
        Event::Text(code.into()),
        Event::End(TagEnd::CodeBlock),
    ])
}

/// `s` with the CRs that end a line dropped, and the other CRs made `lone`.
fn lf(s: &str, lone: char) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut crs = 0;
    for c in s.chars() {
        if c == '\r' {
            crs += 1;
            continue;
        }
        if c != '\n' {
            out.extend(std::iter::repeat_n(lone, crs));
        }
        crs = 0;
        out.push(c);
    }
    out.into()
}

/// What a Ruby value read from YAML is, for printing it and comparing keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Null,
    Bool,
    Int,
    Float,
    Str,
    Sym,
    Time,
    Date,
}

/// A value as Psych makes it: a scalar is kept as its Ruby `to_s`.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Scalar(Kind, String),
    Seq(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

fn scalar(kind: Kind, s: impl Into<String>) -> Value {
    Value::Scalar(kind, s.into())
}

/// Psych's `to_ruby` (with `safe_load`'s merge keys): a mapping is a Hash, where a later
/// duplicate key replaces the value and keeps the first one's place, and `<<` merges a mapping
/// (or a list of them) into it.
fn value(node: &Node) -> Value {
    match node {
        Node::Scalar { text, plain, tag } => {
            let tag = tag.as_deref();
            // Psych's `quoted`: libyaml's quoted_implicit (not set by a `!` tag).
            if !plain && tag.is_none() {
                return scalar(Kind::Str, text.as_str());
            }
            match tag {
                Some("tag:yaml.org,2002:str" | "!str") => scalar(Kind::Str, text.as_str()),
                Some("tag:yaml.org,2002:binary" | "!binary") => scalar(Kind::Str, base64(text)),
                Some("tag:yaml.org,2002:float" | "!float") => match tokenize(text) {
                    Value::Scalar(Kind::Int | Kind::Float, s) => match s.parse::<f64>() {
                        Ok(f) => scalar(Kind::Float, ruby_float(f)),
                        Err(_) => scalar(Kind::Str, text.as_str()),
                    },
                    other => other,
                },
                _ => tokenize(text),
            }
        }
        Node::Seq(items) => Value::Seq(items.iter().map(value).collect()),
        Node::Map(entries) => {
            let mut hash = Hash::default();
            for (k, v) in entries {
                let key = value(k);
                let str_tagged =
                    matches!(k, Node::Scalar { tag: Some(t), .. } if t == "tag:yaml.org,2002:str");
                if matches!(&key, Value::Scalar(Kind::Str, s) if s == "<<") && !str_tagged {
                    match value(v) {
                        Value::Map(m) => {
                            for (k, v) in m {
                                hash.insert(k, v);
                            }
                            continue;
                        }
                        Value::Seq(items) if items.iter().all(|i| matches!(i, Value::Map(_))) => {
                            let mut merged = Hash::default();
                            for item in items.into_iter().rev() {
                                if let Value::Map(m) = item {
                                    for (k, v) in m {
                                        merged.insert(k, v);
                                    }
                                }
                            }
                            for (k, v) in merged.entries {
                                hash.insert(k, v);
                            }
                            continue;
                        }
                        val => {
                            hash.insert(key, val);
                            continue;
                        }
                    }
                }
                hash.insert(key, value(v));
            }
            Value::Map(hash.entries)
        }
        // Never reached: a document with an alias is an error.
        Node::Alias => scalar(Kind::Null, ""),
    }
}

/// Base64 decoded as Ruby's `unpack1("m")` decodes it (other characters skipped, up to a `=`),
/// read as UTF-8: bytes that aren't UTF-8 show as U+FFFD.
fn base64(s: &str) -> String {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6 | v as u32) & 0xfff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A Ruby Hash: insertion order, a key's place kept when its value is replaced.
#[derive(Default)]
struct Hash {
    entries: Vec<(Value, Value)>,
    index: HashMap<(Kind, String), usize>,
}

impl Hash {
    fn insert(&mut self, k: Value, v: Value) {
        let id = match &k {
            Value::Scalar(kind, s) => (*kind, s.clone()),
            other => (Kind::Null, format!("\0{}", inspect(other))),
        };
        match self.index.get(&id) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(id, self.entries.len());
                self.entries.push((k, v));
            }
        }
    }
}

/// Psych's `ScalarScanner#tokenize`: what a plain scalar is.
fn tokenize(s: &str) -> Value {
    if s.is_empty() {
        return scalar(Kind::Null, "");
    }
    let lines = || s.split('\n');
    // The "string" check, anchored at each line's start as Ruby's `^` is.
    let class = |c: char| c.is_alphabetic() || "_ \t\r\n\x0c\x0b!@#$%^&*(){}<>|/\\~;=".contains(c);
    let stringy = lines().any(|l| {
        let mut cs = l.chars();
        match (cs.next(), cs.next()) {
            (Some(a), _) if class(a) => true,
            (Some(a), Some(b)) => !(a.is_ascii_digit() || ".:-".contains(a)) && class(b),
            _ => false,
        }
    }) || s.contains('\n');
    if stringy {
        if s.chars().count() > 5
            || lines().any(|l| l.chars().next().is_some_and(|c| !"ytonf~YTONF".contains(c)))
        {
            return scalar(Kind::Str, s);
        }
        let any = |words: &[&str]| lines().any(|l| words.iter().any(|w| l.eq_ignore_ascii_case(w)));
        if s == "~" || any(&["null"]) {
            return scalar(Kind::Null, "");
        }
        if any(&["yes", "true", "on"]) {
            return scalar(Kind::Bool, "true");
        }
        if any(&["no", "false", "off"]) {
            return scalar(Kind::Bool, "false");
        }
        return scalar(Kind::Str, s);
    }
    if let Some(t) = time(s) {
        return scalar(Kind::Time, t);
    }
    if let Some(d) = date(s) {
        return scalar(Kind::Date, d);
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        ".inf" | "+.inf" => return scalar(Kind::Float, "Infinity"),
        "-.inf" => return scalar(Kind::Float, "-Infinity"),
        ".nan" => return scalar(Kind::Float, "NaN"),
        _ => {}
    }
    if let Some(rest) = s.strip_prefix(':')
        && !rest.is_empty()
    {
        let quoted = rest.len() >= 2
            && matches!(rest.as_bytes()[0], b'"' | b'\'')
            && rest[1..].contains(rest.as_bytes()[0] as char);
        let sym = if quoted {
            // `:"a b"`: up to the last matching quote, as Ruby's greedy `(.*)\1`.
            let q = rest.as_bytes()[0] as char;
            let inner = &rest[1..rest.rfind(q).unwrap_or(1)];
            inner.strip_prefix(':').unwrap_or(inner)
        } else {
            rest
        };
        return scalar(Kind::Sym, sym);
    }
    if let Some(v) = sexagesimal(s) {
        return v;
    }
    if let Some(f) = float(s) {
        return scalar(Kind::Float, f);
    }
    if let Some(i) = integer(s) {
        return scalar(Kind::Int, i);
    }
    scalar(Kind::Str, s)
}

/// `[-+]?[0-9][0-9_]*(:[0-5]?[0-9]){1,2}`, an integer in base 60 (`1:30` is 5400: Psych counts
/// from 60², however many parts there are), or with `\.[0-9_]*` after it a float.
fn sexagesimal(s: &str) -> Option<Value> {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (int_part, frac) = match body.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (body, None),
    };
    if frac.is_some_and(|f| !f.bytes().all(|b| b.is_ascii_digit() || b == b'_')) {
        return None;
    }
    let mut parts = int_part.split(':');
    let first = parts.next()?;
    let fb = first.as_bytes();
    if fb.first().is_none_or(|b| !b.is_ascii_digit())
        || !fb.iter().all(|b| b.is_ascii_digit() || *b == b'_')
    {
        return None;
    }
    // `(:[0-5]?[0-9]){1,2}`: one or two parts of 0 to 59, worth 60 and 1 (Psych counts from 60²).
    let (mut small, mut n, mut last) = (0i64, 0, "");
    for p in parts {
        let v = match *p.as_bytes() {
            [d] if d.is_ascii_digit() => d - b'0',
            [t @ b'0'..=b'5', d] if d.is_ascii_digit() => (t - b'0') * 10 + d - b'0',
            _ => return None,
        };
        if n == 2 {
            return None;
        }
        small += v as i64 * [60, 1][n];
        n += 1;
        last = p;
    }
    if n == 0 {
        return None;
    }
    let neg = s.starts_with('-');
    match frac {
        None => {
            let big = ruby_to_i(first)?.checked_mul(3600)?;
            let total = if neg { -big } else { big }.checked_add(small as i128)?;
            Some(scalar(Kind::Int, total.to_string()))
        }
        Some(f) => {
            // As Ruby: each part's `to_f` times its weight, the last one with the fraction.
            let num = |t: &str| {
                let t: String = t.chars().filter(|&c| c != '_').collect();
                t.parse::<f64>().ok()
            };
            let mut total = num(first)? * 3600.0;
            if neg {
                total = -total;
            }
            for (k, weight) in [60.0, 1.0].into_iter().enumerate().take(n) {
                let v = if k + 1 == n {
                    num(&format!("{last}.{f}"))?
                } else {
                    (small / 60) as f64
                };
                total += v * weight;
            }
            Some(scalar(Kind::Float, ruby_float(total)))
        }
    }
}

/// Ruby's `String#to_i` of a run of digits with single underscores in between.
fn ruby_to_i(s: &str) -> Option<i128> {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i128 = 0;
    let mut prev_us = false;
    for b in digits.bytes() {
        match b {
            b'0'..=b'9' => {
                n = n.checked_mul(10)?.checked_add((b - b'0') as i128)?;
                prev_us = false;
            }
            b'_' if !prev_us => prev_us = true,
            _ => break,
        }
    }
    Some(if neg { -n } else { n })
}

/// `[-+]?([0-9][0-9_,]*)?\.[0-9]*([eE][-+][0-9]+)?` (but not a lone `.`), printed as Ruby
/// prints the Float.
fn float(s: &str) -> Option<String> {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (int_part, rest) = body.split_once('.')?;
    let ib = int_part.as_bytes();
    if !ib.is_empty()
        && (!ib[0].is_ascii_digit()
            || !ib
                .iter()
                .all(|b| b.is_ascii_digit() || *b == b'_' || *b == b','))
    {
        return None;
    }
    let (frac, exp) = match rest.find(['e', 'E']) {
        Some(p) => (&rest[..p], Some(&rest[p + 1..])),
        None => (rest, None),
    };
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(e) = exp {
        let digits = e.strip_prefix(['-', '+'])?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    if int_part.is_empty() && frac.is_empty() && exp.is_none() {
        return None;
    }
    // `1.` and `1.e5` lose their dot.
    let t: String = s
        .chars()
        .filter(|&c| c != ',' && c != '_' && (c != '.' || !frac.is_empty()))
        .collect();
    Some(ruby_float(t.parse::<f64>().ok()?))
}

/// Psych's legacy integer forms (`0b`, octal `0…`, decimal with `_` and `,`, `0x`), in decimal.
fn integer(s: &str) -> Option<String> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let sep = |b: u8| b == b'_' || b == b',';
    // `[_,]*` then a digit, then digits and separators.
    let radix_body = |digits: &str, ok: fn(u8) -> bool| {
        let b = digits.as_bytes();
        let first = b.iter().position(|&c| !sep(c))?;
        (ok(b[first]) && b[first..].iter().all(|&c| ok(c) || sep(c))).then_some(())
    };
    let (radix, digits) = if let Some(d) = body.strip_prefix("0b") {
        radix_body(d, |c| c == b'0' || c == b'1')?;
        (2, d)
    } else if let Some(d) = body.strip_prefix("0x") {
        radix_body(d, |c| c.is_ascii_hexdigit())?;
        (16, d)
    } else if body == "0" {
        (10, body)
    } else if let Some(d) = body.strip_prefix('0') {
        radix_body(d, |c| (b'0'..=b'7').contains(&c))?;
        (8, d)
    } else {
        // `[1-9](?:[0-9]|,[0-9]|_[0-9])*`
        let b = body.as_bytes();
        if b.is_empty() || !(b'1'..=b'9').contains(&b[0]) {
            return None;
        }
        let mut prev_sep = false;
        for &c in &b[1..] {
            match c {
                b'0'..=b'9' => prev_sep = false,
                _ if sep(c) && !prev_sep => prev_sep = true,
                _ => return None,
            }
        }
        if prev_sep {
            return None;
        }
        (10, body)
    };
    if radix == 10 {
        let digits: String = digits.chars().filter(|&c| c != '_' && c != ',').collect();
        return Some(if neg && digits != "0" {
            format!("-{digits}")
        } else {
            digits
        });
    }
    let digits: Vec<u32> = digits
        .bytes()
        .filter(|&c| !sep(c))
        .map(|c| (c as char).to_digit(16).unwrap_or(0))
        .collect();
    if radix != 10 && digits.len() > MAX_RADIX_DIGITS {
        return None;
    }
    // Little-endian limbs of 10⁹.
    let mut limbs: Vec<u64> = vec![0];
    for d in digits {
        let mut carry = d as u64;
        for l in &mut limbs {
            let v = *l * radix + carry;
            *l = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut out = String::new();
    if neg && limbs != [0] {
        out.push('-');
    }
    for (k, &l) in limbs.iter().rev().enumerate() {
        let mut buf = [b'0'; 9];
        let mut v = l;
        for d in buf.iter_mut().rev() {
            *d = b'0' + (v % 10) as u8;
            v /= 10;
        }
        // The first limb without its leading zeros.
        let from = if k == 0 {
            buf.iter().take(8).take_while(|&&d| d == b'0').count()
        } else {
            0
        };
        out.extend(buf[from..].iter().map(|&d| d as char));
    }
    Some(out)
}

/// Ruby's `Float#to_s`: the shortest digits that read back as the same number, with a `.0` on
/// whole numbers, in exponent form below 0.0001 and from 10¹⁶ up.
fn ruby_float(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // Rust's Debug form switches to an exponent at the same bounds as Ruby (below 1e-4, from 1e16).
    let s = format!("{x:?}");
    match s.split_once('e') {
        None => s,
        Some((m, e)) => {
            let dot = if m.contains('.') { "" } else { ".0" };
            format!("{m}{dot}e{:+03}", e.parse::<i32>().unwrap_or(0))
        }
    }
}

/// Psych's `Date` form, `\d{4}-\d{1,2}-\d{1,2}` (a real date), printed `YYYY-MM-DD`.
fn date(s: &str) -> Option<String> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some()
        || y.len() != 4
        || !(1..=2).contains(&m.len())
        || !(1..=2).contains(&d.len())
        || ![y, m, d]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let (y, m, d) = (num(y), num(m) as u32, num(d) as u32);
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(format!("{:04}-{m:02}-{d:02}", y as u32))
}

/// The number written in ASCII digits `s` (already checked, and short).
fn num(s: &str) -> i64 {
    s.bytes().fold(0, |n, b| n * 10 + (b - b'0') as i64)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the date (proleptic Gregorian; month and day may run over).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Psych's timestamp, `-?\d{4}-\d{1,2}-\d{1,2}([Tt]|\s+)\d{1,2}:\d\d:\d\d(\.\d*)?` and an
/// optional zone (`Z`, `±h`, `±hh:mm`, `±hhmm`), printed as Ruby's `Time#to_s`: with its zone
/// as `±hhmm`, `UTC` for `Z`, and without a zone read as UTC and shown in GitHub's server zone,
/// America/Los_Angeles.
fn time(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize, min: usize, max: usize| -> Option<i64> {
        let from = *i;
        while *i < b.len() && *i - from < max && b[*i].is_ascii_digit() {
            *i += 1;
        }
        (*i - from >= min).then(|| num(&s[from..*i]))
    };
    let neg = b.first() == Some(&b'-');
    i += neg as usize;
    let year = digits(&mut i, 4, 4)?;
    let year = if neg { -year } else { year };
    let lit = |i: &mut usize, c: u8| (b.get(*i) == Some(&c)).then(|| *i += 1);
    lit(&mut i, b'-')?;
    let month = digits(&mut i, 1, 2)?;
    lit(&mut i, b'-')?;
    let day = digits(&mut i, 1, 2)?;
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\x0b');
    match b.get(i) {
        Some(b'T' | b't') => i += 1,
        Some(&c) if ws(c) => {
            while b.get(i).is_some_and(|&c| ws(c)) {
                i += 1;
            }
        }
        _ => return None,
    }
    let hour = digits(&mut i, 1, 2)?;
    lit(&mut i, b':')?;
    let min = digits(&mut i, 2, 2)?;
    lit(&mut i, b':')?;
    let sec = digits(&mut i, 2, 2)?;
    if lit(&mut i, b'.').is_some() {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    while b.get(i).is_some_and(|&c| ws(c)) {
        i += 1;
    }
    // The zone: none, `Z`, or `[-+]\d{1,2}:?(\d\d)?`.
    let zone = if i == b.len() {
        None
    } else if &s[i..] == "Z" {
        Some(None)
    } else {
        let sign = match b[i] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        i += 1;
        // `\d{1,2}:?(\d\d)?`, then split as Psych splits it: the first one or two digits are the
        // hours, the rest the minutes.
        let from = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let (h, m) = if b.get(i) == Some(&b':') {
            let (h, rest) = (&s[from..i], &s[i + 1..]);
            if !(1..=2).contains(&h.len())
                || !(rest.is_empty()
                    || (rest.len() == 2 && rest.bytes().all(|c| c.is_ascii_digit())))
            {
                return None;
            }
            (h, rest)
        } else {
            let all = &s[from..i];
            if i != b.len() || all.is_empty() || all.len() > 4 {
                return None;
            }
            all.split_at(all.len().min(2))
        };
        let (h, m) = (num(h), num(m));
        // The hours carry the sign (so `-00:30` is +00:30), the minutes follow it.
        let hours = sign * h * 3600;
        Some(Some(if hours < 0 {
            hours - m * 60
        } else {
            hours + m * 60
        }))
    };
    // Ruby's `Time.utc`/`Time.new` ranges; a day past the month's end runs into the next.
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 24
        || (hour == 24 && (min > 0 || sec > 0))
        || min > 59
        || sec > 60
    {
        return None;
    }
    let wall =
        days_from_civil(year, month as u32, day as u32) * 86_400 + hour * 3600 + min * 60 + sec;
    let (shown, offset, utc) = match zone {
        Some(None) => (wall, 0, true),
        Some(Some(off)) => {
            if off.abs() >= 86_400 {
                return None;
            }
            (wall, off, false)
        }
        None => {
            let off = los_angeles(wall);
            (wall + off, off, false)
        }
    };
    let days = shown.div_euclid(86_400);
    let t = shown.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    let (t, a) = (t as u32, offset.unsigned_abs() as u32);
    let mut out = format!(
        "{}{:04}-{mo:02}-{d:02} {:02}:{:02}:{:02} ",
        if y < 0 { "-" } else { "" },
        y.unsigned_abs() as u32,
        t / 3600,
        t % 3600 / 60,
        t % 60
    );
    if utc {
        out.push_str("UTC");
    } else {
        let sign = if offset < 0 { '-' } else { '+' };
        out.push_str(&format!("{sign}{:02}{:02}", a / 3600, a % 3600 / 60));
    }
    Some(out)
}

/// America/Los_Angeles's offset from UTC at the instant `utc` (seconds since 1970), by the US
/// rules: since 2007 daylight time from the second Sunday of March to the first Sunday of
/// November, from 1987 to 2006 from the first Sunday of April to the last Sunday of October (at
/// 2:00 local time). Earlier years are given the 1987 rule (the zone's older history isn't kept).
fn los_angeles(utc: i64) -> i64 {
    let (y, _, _) = civil_from_days(utc.div_euclid(86_400));
    // The day of the `n`th Sunday of month `m` (n = 0: the last one).
    let sunday = |m: u32, n: i64| {
        let first = days_from_civil(y, m, 1);
        // 1970-01-01 was a Thursday: (days + 4) mod 7 is the weekday, 0 = Sunday.
        let to_sunday = (7 - (first + 4).rem_euclid(7)) % 7;
        if n > 0 {
            first + to_sunday + 7 * (n - 1)
        } else {
            let last = first + days_in_month(y, m) as i64 - 1;
            last - (last + 4).rem_euclid(7)
        }
    };
    let (start, end) = if y >= 2007 {
        (sunday(3, 2), sunday(11, 1))
    } else {
        (sunday(4, 1), sunday(10, 0))
    };
    // 2:00 PST is 10:00 UTC; 2:00 PDT is 9:00 UTC.
    let dst = (start * 86_400 + 10 * 3600..end * 86_400 + 9 * 3600).contains(&utc);
    if dst { -7 * 3600 } else { -8 * 3600 }
}

/// Ruby's `inspect`, which GitHub shows for a key that isn't a scalar (`["x"]`).
fn inspect(v: &Value) -> String {
    match v {
        Value::Scalar(Kind::Str, s) => {
            let mut out = String::from("\"");
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    '\x1b' => out.push_str("\\e"),
                    '\x07' => out.push_str("\\a"),
                    '\x08' => out.push_str("\\b"),
                    '\x0b' => out.push_str("\\v"),
                    '\x0c' => out.push_str("\\f"),
                    '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
                    c if (c as u32) < 0x20 || c == '\x7f' => {
                        out.push_str(&format!("\\u{:04X}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        Value::Scalar(Kind::Null, _) => "nil".into(),
        Value::Scalar(Kind::Sym, s) => {
            let plain = s
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_alphanumeric() || c == '_');
            if plain {
                format!(":{s}")
            } else {
                format!(":{}", inspect(&scalar(Kind::Str, s.as_str())))
            }
        }
        Value::Scalar(_, s) => s.clone(),
        Value::Seq(items) => {
            let items: Vec<String> = items.iter().map(inspect).collect();
            format!("[{}]", items.join(", "))
        }
        // Ruby 3.4's form.
        Value::Map(entries) => {
            let entries: Vec<String> = entries
                .iter()
                .map(|(k, v)| match k {
                    Value::Scalar(Kind::Sym, s) if inspect(k) == format!(":{s}") => {
                        format!("{s}: {}", inspect(v))
                    }
                    _ => format!("{} => {}", inspect(k), inspect(v)),
                })
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

/// What the browser shows of a cell's text: runs of whitespace as one space, none at the ends
/// (GitHub puts values in the cell as they are, line breaks included).
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s
        .split(['\t', '\n', '\x0c', '\r', ' '])
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().map(|c| if c == '\0' { '\u{fffd}' } else { c }));
    }
    out
}

/// How a key or a scalar reads in its cell: Ruby's `to_s` (nil is empty), a key that isn't a
/// scalar as its `inspect`.
fn shown(v: &Value) -> String {
    match v {
        Value::Scalar(_, s) => collapse(s),
        other => collapse(&inspect(other)),
    }
}

/// The front matter's table: a row per key, the key in bold (GitHub's `<th>`), its value in the
/// next cell.
fn table(map: &[(Value, Value)]) -> Vec<Event<'static>> {
    let mut out = vec![Event::Start(Tag::Table(vec![Alignment::None; 2]))];
    for (k, v) in map {
        out.push(Event::Start(Tag::TableRow));
        out.push(Event::Start(Tag::TableCell));
        strong(&mut out, shown(k));
        out.push(Event::End(TagEnd::TableCell));
        out.push(Event::Start(Tag::TableCell));
        let mut cell = Cell {
            out: &mut out,
            fresh: true,
        };
        cell.value(v, 0);
        out.push(Event::End(TagEnd::TableCell));
        out.push(Event::End(TagEnd::TableRow));
    }
    out.push(Event::End(TagEnd::Table));
    out
}

fn text(out: &mut Vec<Event<'static>>, s: String) {
    if !s.is_empty() {
        out.push(Event::Text(CowStr::from(s)));
    }
}

fn strong(out: &mut Vec<Event<'static>>, s: String) {
    if !s.is_empty() {
        out.push(Event::Start(Tag::Strong));
        out.push(Event::Text(CowStr::from(s)));
        out.push(Event::End(TagEnd::Strong));
    }
}

/// A value cell's outline: a map is a line per entry, `key: value`; a list of scalars is one
/// line, `a, b, c` (GitHub's row of cells); any other list is a line per item, after a bullet.
/// What is nested goes on the lines below, indented.
struct Cell<'a> {
    out: &'a mut Vec<Event<'static>>,
    /// Nothing is in the cell yet.
    fresh: bool,
}

/// GitHub draws no cell for a nil item.
fn items(items: &[Value]) -> impl Iterator<Item = &Value> {
    items
        .iter()
        .filter(|v| !matches!(v, Value::Scalar(Kind::Null, _)))
}

impl Cell<'_> {
    /// A new line at nesting `level` (none for the cell's first).
    fn line(&mut self, level: usize) {
        if !self.fresh {
            self.out.push(Event::HardBreak);
        }
        self.fresh = false;
        if level > 0 {
            text(self.out, "\u{a0}\u{a0}".repeat(level));
        }
    }

    /// What fits on the line it starts on (a scalar, a list of scalars, nothing for an empty
    /// collection), or `false` for a collection that needs lines of its own.
    fn inline(&mut self, v: &Value) -> bool {
        match v {
            Value::Scalar(..) => text(self.out, shown(v)),
            Value::Seq(list) if items(list).all(|v| matches!(v, Value::Scalar(..))) => {
                let parts: Vec<String> = items(list).map(shown).collect();
                text(self.out, parts.join(", "));
            }
            Value::Map(m) if m.is_empty() => {}
            _ => return false,
        }
        self.fresh = false;
        true
    }

    /// `v` at nesting `level`: inline if it fits, else on lines of its own.
    fn value(&mut self, v: &Value, level: usize) {
        if !self.inline(v) {
            self.lines(v, level, self.fresh);
        }
    }

    /// A collection's lines at `level`; `here`: its first line goes on the current one.
    fn lines(&mut self, v: &Value, level: usize, mut here: bool) {
        match v {
            Value::Map(m) => {
                for (k, val) in m {
                    if !std::mem::take(&mut here) {
                        self.line(level);
                    }
                    self.fresh = false;
                    strong(self.out, shown(k));
                    text(self.out, ": ".into());
                    if !self.inline(val) {
                        self.lines(val, level + 1, false);
                    }
                }
            }
            Value::Seq(list) => {
                for item in items(list) {
                    if !std::mem::take(&mut here) {
                        self.line(level);
                    }
                    self.fresh = false;
                    text(self.out, "\u{2022} ".into());
                    if !self.inline(item) {
                        self.lines(item, level + 1, true);
                    }
                }
            }
            Value::Scalar(..) => {
                self.inline(v);
            }
        }
    }
}

#[cfg(test)]
mod tests;
