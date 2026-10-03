//! Secrets out of text before base writes it to disk (K1d, BO-13).
//!
//! The match log keeps what a prompt said, so a prompt that pasted a key would keep the key for 90 days. Every secret
//! this module recognises becomes `[SECRET:<kind>]` before anything is written. No regex crate: each shape is a small
//! scanner over bytes. Every shape starts and ends on an ASCII byte, so a span always falls on a char boundary.
//!
//! The shapes, and the kind each becomes:
//!
//! | shape | kind |
//! |---|---|
//! | `sk-ant-` and 10 or more key characters | `anthropic-key` |
//! | `sk-` and 20 or more key characters, a digit among them | `openai-key` |
//! | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` and 20 or more letters or digits; `github_pat_` and 20 or more | `github-token` |
//! | `AKIA` or `ASIA` and exactly 16 capitals or digits | `aws-key` |
//! | `xoxb-`, `xoxp-` (and `xoxa-`, `xoxr-`, `xoxs-`) and 10 or more | `slack-token` |
//! | `Bearer` and a token of 16 or more characters (the token only) | `bearer-token` |
//! | `password` or `passwd`, then `=` or `:`, then the value (the value only) | `password` |
//! | the password in a URL, `scheme://user:<password>@host` (the password only) | `password` |
//! | a credential's name, then `=`, then the value (the value only): a name ending in `_key`, `_token`, `_secret`, `_password`, `_passwd` (or `-key` ...), or `api_key`, `apikey`, `token`, `secret`. `:` counts too when the name is quoted (`"client_secret": "..."`) or has a `_` or `-` in it (`aws_secret_access_key: ...`) | `credential` |
//! | `-----BEGIN ... PRIVATE KEY-----` through its `-----END ...-----` line, or to the end when there is none | `private-key` |
//! | `eyJ...` `.` `...` `.` `...` (a JWT's three base64url parts) | `jwt` |
//!
//! A key shape must start where no key character (letter, digit, `-`, `_`) comes before it, so `task-...` never reads
//! as `sk-...`. A password or a credential's name only needs no letter, digit or `_` before it, so `--password=x` and
//! `db-password=x` are caught. Where two spans overlap they become one, named by the one that starts first, so no part
//! of either is left.

/// `text` with every secret replaced by `[SECRET:<kind>]`.
pub fn scrub(text: &str) -> String {
    let spans = find(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, kind) in spans {
        out.push_str(&text[at..start]);
        out.push_str(&format!("[SECRET:{kind}]"));
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// The kinds of secret `scrub` would replace in `text`, in order. For tests and for saying what a scrub did.
pub fn kinds(text: &str) -> Vec<&'static str> {
    find(text).into_iter().map(|(_, _, k)| k).collect()
}

/// Every secret span, sorted, none overlapping: (start, end, kind).
fn find(text: &str) -> Vec<(usize, usize, &'static str)> {
    let b = text.as_bytes();
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    private_keys(text, &mut spans);
    for i in 0..b.len() {
        if let Some((s, e)) = url_password(b, i) {
            spans.push((s, e, "password"));
        }
        if word_start(b, i) {
            if let Some((s, e)) = assigned(b, i, &[b"password", b"passwd"], true) {
                spans.push((s, e, "password"));
            } else if let Some((s, e)) = credential(b, i) {
                spans.push((s, e, "credential"));
            }
        }
        if !boundary_before(b, i) {
            continue;
        }
        let found = anthropic(b, i)
            .map(|e| (e, "anthropic-key"))
            .or_else(|| openai(b, i).map(|e| (e, "openai-key")))
            .or_else(|| github(b, i).map(|e| (e, "github-token")))
            .or_else(|| aws(b, i).map(|e| (e, "aws-key")))
            .or_else(|| slack(b, i).map(|e| (e, "slack-token")))
            .or_else(|| jwt(b, i).map(|e| (e, "jwt")));
        if let Some((end, kind)) = found {
            spans.push((i, end, kind));
        } else if let Some((s, e)) = bearer(b, i) {
            spans.push((s, e, "bearer-token"));
        }
    }
    // Earliest first, and at one start the first found. Overlapping spans merge, so a secret that starts inside
    // another (a key block after `TOKEN=abc`) is covered to its own end.
    spans.sort_by_key(|(s, _, _)| *s);
    let mut kept: Vec<(usize, usize, &'static str)> = Vec::new();
    for span in spans {
        match kept.last_mut() {
            Some(last) if span.0 < last.1 => last.1 = last.1.max(span.1),
            _ => kept.push(span),
        }
    }
    kept
}

/// A key's own characters: letters, digits, `-` and `_`.
fn is_key(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// Nothing that could be part of the same key or word sits right before `i`.
fn boundary_before(b: &[u8], i: usize) -> bool {
    i == 0 || !is_key(b[i - 1])
}

/// No letter, digit or `_` right before `i`: the start of a word, which may follow a `-` (`--password`).
fn word_start(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
}

/// How many bytes from `i` satisfy `ok`.
fn run(b: &[u8], i: usize, ok: impl Fn(u8) -> bool) -> usize {
    b[i.min(b.len())..].iter().take_while(|c| ok(**c)).count()
}

fn starts(b: &[u8], i: usize, prefix: &[u8]) -> bool {
    b.len() >= i + prefix.len() && &b[i..i + prefix.len()] == prefix
}

fn starts_ci(b: &[u8], i: usize, prefix: &[u8]) -> bool {
    b.len() >= i + prefix.len() && b[i..i + prefix.len()].eq_ignore_ascii_case(prefix)
}

fn anthropic(b: &[u8], i: usize) -> Option<usize> {
    let p = b"sk-ant-".len();
    (starts(b, i, b"sk-ant-") && run(b, i + p, is_key) >= 10).then(|| i + p + run(b, i + p, is_key))
}

fn openai(b: &[u8], i: usize) -> Option<usize> {
    if !starts(b, i, b"sk-") || starts(b, i, b"sk-ant-") {
        return None;
    }
    let n = run(b, i + 3, is_key);
    let digit = b[i + 3..i + 3 + n].iter().any(u8::is_ascii_digit);
    (n >= 20 && digit).then_some(i + 3 + n)
}

fn github(b: &[u8], i: usize) -> Option<usize> {
    if starts(b, i, b"github_pat_") {
        let n = run(b, i + 11, |c| c.is_ascii_alphanumeric() || c == b'_');
        return (n >= 20).then_some(i + 11 + n);
    }
    let classic = b.len() >= i + 4 && &b[i..i + 2] == b"gh" && b"pousr".contains(&b[i + 2]) && b[i + 3] == b'_';
    if !classic {
        return None;
    }
    let n = run(b, i + 4, |c| c.is_ascii_alphanumeric());
    (n >= 20).then_some(i + 4 + n)
}

fn aws(b: &[u8], i: usize) -> Option<usize> {
    if !(starts(b, i, b"AKIA") || starts(b, i, b"ASIA")) {
        return None;
    }
    let n = run(b, i + 4, |c| c.is_ascii_uppercase() || c.is_ascii_digit());
    // Exactly sixteen, and nothing of a longer word after them.
    (n == 16 && b.get(i + 20).is_none_or(|c| !is_key(*c))).then_some(i + 20)
}

fn slack(b: &[u8], i: usize) -> Option<usize> {
    let shaped = b.len() >= i + 5 && &b[i..i + 3] == b"xox" && b"bpars".contains(&b[i + 3]) && b[i + 4] == b'-';
    if !shaped {
        return None;
    }
    let n = run(b, i + 5, |c| c.is_ascii_alphanumeric() || c == b'-');
    (n >= 10).then_some(i + 5 + n)
}

/// `eyJ<10+>.<10+>.<0+>` in base64url: a JWT's header, payload and signature.
fn jwt(b: &[u8], i: usize) -> Option<usize> {
    let part = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
    if !starts(b, i, b"eyJ") {
        return None;
    }
    let h = run(b, i, part);
    if h < 10 || b.get(i + h) != Some(&b'.') {
        return None;
    }
    let p_at = i + h + 1;
    let p = run(b, p_at, part);
    if p < 10 || b.get(p_at + p) != Some(&b'.') {
        return None;
    }
    let s_at = p_at + p + 1;
    Some(s_at + run(b, s_at, part))
}

/// `Bearer <token>`: the token's span only, so the reader still sees an Authorization header was there.
fn bearer(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !starts_ci(b, i, b"bearer") {
        return None;
    }
    let gap = run(b, i + 6, |c| c == b' ' || c == b'\t');
    if gap == 0 {
        return None;
    }
    let at = i + 6 + gap;
    // A token's characters: letters, digits and the `-._~+/=` RFC 6750 allows, and `_` for base64url.
    let n = run(b, at, |c| c.is_ascii_alphanumeric() || b"-._~+/=_".contains(&c));
    (n >= 16).then_some((at, at + n))
}

/// `<key>=value` or, when `colon`, `<key>: value`, for one of `keys` (case ignored), with an optional closing quote
/// after the key (`"password": "x"`) and an optional quote around the value. The value's span only.
fn assigned(b: &[u8], i: usize, keys: &[&[u8]], colon: bool) -> Option<(usize, usize)> {
    let key = keys.iter().find(|k| starts_ci(b, i, k))?;
    let mut at = i + key.len();
    if b.get(at).is_some_and(|c| is_key(*c)) {
        return None;
    }
    if matches!(b.get(at), Some(b'"' | b'\'')) {
        at += 1;
    }
    value_after(b, at, colon)
}

/// After a key ends at `at`: optional spaces, `=` (or `:` when `colon`), optional spaces, then the value, quoted or not.
fn value_after(b: &[u8], mut at: usize, colon: bool) -> Option<(usize, usize)> {
    at += run(b, at, |c| c == b' ' || c == b'\t');
    match b.get(at) {
        Some(b'=') => {}
        Some(b':') if colon => {}
        _ => return None,
    }
    at += 1;
    at += run(b, at, |c| c == b' ' || c == b'\t');
    let (start, end) = match b.get(at) {
        Some(q @ (b'"' | b'\'')) => {
            let n = run(b, at + 1, |c| c != *q && c != b'\n');
            (at + 1, at + 1 + n)
        }
        _ => (at, at + run(b, at, |c| !c.is_ascii_whitespace() && !b",;&\"'".contains(&c))),
    };
    (end > start).then_some((start, end))
}

/// `NAME=value` where the name is a credential's: ends in `_key`, `_token`, `_secret`, `_password`, `_passwd` (or the
/// same after `-`), or is `api_key`, `apikey`, `token`, `secret`. A colon also counts when the name is quoted or has a
/// `_` or `-` in it: `"client_secret": "..."`, `aws_secret_access_key: ...`. After a bare word (`token: ...`) a colon
/// is ordinary prose far too often.
fn credential(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !b.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let n = run(b, i, is_key);
    let name = b[i..i + n].to_ascii_lowercase();
    let whole = [b"api_key".as_slice(), b"apikey", b"api-key", b"token", b"secret"];
    let tails = [b"key".as_slice(), b"token", b"secret", b"password", b"passwd"];
    let tailed = tails.iter().any(|t| {
        name.len() > t.len() + 1 && name.ends_with(t) && matches!(name[name.len() - t.len() - 1], b'_' | b'-')
    });
    if !(whole.contains(&name.as_slice()) || tailed) {
        return None;
    }
    let quote = i.checked_sub(1).map(|q| b[q]).filter(|q| matches!(q, b'"' | b'\'') && b.get(i + n) == Some(q));
    let colon = quote.is_some() || name.contains(&b'_') || name.contains(&b'-');
    value_after(b, i + n + usize::from(quote.is_some()), colon)
}

/// `scheme://user:password@host`: the password's span, when `i` is at the `://`.
fn url_password(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !starts(b, i, b"://") {
        return None;
    }
    let from = i + 3;
    let len = run(b, from, |c| !c.is_ascii_whitespace() && !b"/?#\"'<>".contains(&c));
    let authority = &b[from..from + len];
    let at = authority.iter().rposition(|c| *c == b'@')?;
    let colon = authority[..at].iter().position(|c| *c == b':')?;
    (colon + 1 < at).then_some((from + colon + 1, from + at))
}

/// `-----BEGIN ... PRIVATE KEY...-----` through the matching `-----END ...-----`, or to the end of the text.
fn private_keys(text: &str, spans: &mut Vec<(usize, usize, &'static str)>) {
    let mut from = 0;
    while let Some(pos) = text[from..].find("-----BEGIN ") {
        let start = from + pos;
        let head_end = text[start + 11..].find("-----").map(|p| start + 11 + p);
        let Some(head_end) = head_end.filter(|e| text[start..*e].contains("PRIVATE KEY")) else {
            from = start + 11;
            continue;
        };
        let end = text[head_end + 5..]
            .find("-----END ")
            .and_then(|p| {
                let at = head_end + 5 + p + 9;
                text[at..].find("-----").map(|q| at + q + 5)
            })
            .unwrap_or(text.len());
        spans.push((start, end, "private-key"));
        from = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_untouched() {
        for t in [
            "draft the morning update for Anthony",
            "the risk-assessment-for-the-new-project-2026 doc",
            "task-1234567890abcdefghij",
            "a bearer token is not shown here",
            "the password field must be at least 12 characters",
            "sk-learn-tutorial-notes-for-the-team",
            "AKIAN is not a key",
            "Here is a header: eyJ.short.x",
            "my key: value",
        ] {
            assert_eq!(scrub(t), t, "{t}");
        }
    }

    #[test]
    fn review_cases_are_caught() {
        assert_eq!(scrub("mysql -u root --password=hunter2"), "mysql -u root --password=[SECRET:password]");
        assert_eq!(scrub("set db-password=hunter2 and"), "set db-password=[SECRET:credential] and");
        assert_eq!(scrub("curl --client-secret=abc123 x"), "curl --client-secret=[SECRET:credential] x");
        assert_eq!(scrub("{\"client_secret\": \"9f8e7d\"}"), "{\"client_secret\": \"[SECRET:credential]\"}");
        assert_eq!(scrub("aws_secret_access_key: wJalr/K7MDENG"), "aws_secret_access_key: [SECRET:credential]");
        assert_eq!(scrub("postgres://admin:S3cretPass@db/x"), "postgres://admin:[SECRET:password]@db/x");
        assert_eq!(scrub("TOKEN=abc-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY----- ok"), "TOKEN=[SECRET:credential] ok");
        for plain in ["token: the thing we pass", "secret: keep it", "see https://example.com/a:b@c", "http://host:8080/x"] {
            assert_eq!(scrub(plain), plain, "{plain}");
        }
    }

    #[test]
    fn spans_meet_at_the_seams() {
        assert_eq!(scrub("a sk-ant-api03-AbCdEf123456789, b"), "a [SECRET:anthropic-key], b");
        assert_eq!(scrub("password=hunter2 and more"), "password=[SECRET:password] and more");
        assert_eq!(scrub("Authorization: Bearer abcdefghijklmnop1234"), "Authorization: Bearer [SECRET:bearer-token]");
        assert_eq!(kinds("x=1 sk-ant-api03-AbCdEf1234567890 ghp_abcdefghijklmnopqrstuvwxyz0123456789"), ["anthropic-key", "github-token"]);
    }
}
