//! Adding base's hook entries to a host's `settings.json` as text, so every other byte of the file stays as it was
//! (BO-17, lynx's G0 ruling on question 5).
//!
//! WHY NOT PARSE AND WRITE BACK. `serde_json` here keeps object keys sorted (no `preserve_order`), and a pretty print
//! has its own spacing, so writing a parsed file back rewrites every key the user and Claude Code put there in another
//! order. The file is the user's: their own hooks, permissions and settings live in it. So the new entries are inserted
//! as text at the right place, indented like their neighbours, and the result is parsed again and compared with the
//! parsed original plus exactly the new entries. Anything else, and nothing is written.

use serde_json::Value;

/// One JSON value's span in `text`: `start..end` byte offsets.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

struct Scanner<'a> {
    b: &'a [u8],
}

impl<'a> Scanner<'a> {
    fn ws(&self, mut i: usize) -> usize {
        while i < self.b.len() && matches!(self.b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        i
    }

    /// The end of the string starting at `i` (a `"`), past its closing quote.
    fn string(&self, i: usize) -> Option<usize> {
        if self.b.get(i) != Some(&b'"') {
            return None;
        }
        let mut j = i + 1;
        while j < self.b.len() {
            match self.b[j] {
                b'\\' => j += 2,
                b'"' => return Some(j + 1),
                _ => j += 1,
            }
        }
        None
    }

    /// The span of the value starting at `i` (after whitespace).
    fn value(&self, i: usize) -> Option<Span> {
        let start = self.ws(i);
        let end = match *self.b.get(start)? {
            b'"' => self.string(start)?,
            b'{' | b'[' => self.container(start)?,
            _ => {
                let mut j = start;
                while j < self.b.len() && !matches!(self.b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if j == start {
                    return None;
                }
                j
            }
        };
        Some(Span { start, end })
    }

    /// The end of the object or array starting at `i`, past its closing bracket.
    fn container(&self, i: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = i;
        while j < self.b.len() {
            match self.b[j] {
                b'"' => {
                    j = self.string(j)?;
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j + 1);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    /// The members of the object spanning `obj`: each key's text, the key's start and its value's span.
    fn members(&self, obj: Span) -> Option<Vec<(String, usize, Span)>> {
        if self.b.get(obj.start) != Some(&b'{') {
            return None;
        }
        let mut out = Vec::new();
        let mut i = self.ws(obj.start + 1);
        if self.b.get(i) == Some(&b'}') {
            return Some(out);
        }
        loop {
            let key_start = i;
            let key_end = self.string(i)?;
            let key: String = serde_json::from_slice(&self.b[key_start..key_end]).ok()?;
            i = self.ws(key_end);
            if self.b.get(i) != Some(&b':') {
                return None;
            }
            let v = self.value(i + 1)?;
            out.push((key, key_start, v));
            i = self.ws(v.end);
            match self.b.get(i)? {
                b',' => i = self.ws(i + 1),
                b'}' => return Some(out),
                _ => return None,
            }
        }
    }

    /// The elements of the array spanning `arr`.
    fn elements(&self, arr: Span) -> Option<Vec<Span>> {
        if self.b.get(arr.start) != Some(&b'[') {
            return None;
        }
        let mut out = Vec::new();
        let mut i = self.ws(arr.start + 1);
        if self.b.get(i) == Some(&b']') {
            return Some(out);
        }
        loop {
            let v = self.value(i)?;
            out.push(v);
            i = self.ws(v.end);
            match self.b.get(i)? {
                b',' => i = self.ws(i + 1),
                b']' => return Some(out),
                _ => return None,
            }
        }
    }

    /// The spaces and tabs before `at` on its line, when only those stand between the line's start and it.
    fn indent_of(&self, at: usize) -> Option<String> {
        let line_start = self.b[..at].iter().rposition(|c| *c == b'\n').map_or(0, |p| p + 1);
        let lead = &self.b[line_start..at];
        lead.iter().all(|c| *c == b' ' || *c == b'\t').then(|| String::from_utf8_lossy(lead).into_owned())
    }
}

/// `value` pretty-printed, every line after the first indented by `indent`.
fn pretty(value: &Value, indent: &str, unit: &str) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let mut out = String::new();
    for (n, line) in text.lines().enumerate() {
        if n > 0 {
            out.push('\n');
            out.push_str(indent);
        }
        // serde's own unit is two spaces; the file's may differ.
        let lead = line.len() - line.trim_start_matches(' ').len();
        out.push_str(&unit.repeat(lead / 2));
        out.push_str(line.trim_start_matches(' '));
    }
    out
}

/// One insertion: at byte `at`, this text.
struct Insert {
    at: usize,
    text: String,
}

/// `text` (a settings file) with `entries` (event and the object to append to `hooks[event]`) added, every other byte
/// left as it was. `None` when the file is not an object this can read, or the result would not parse back to the
/// original plus exactly these entries: the caller then writes nothing.
pub fn add_hook_entries(text: &str, entries: &[(&str, Value)]) -> Option<String> {
    let original: Value = serde_json::from_str(text).ok()?;
    let sc = Scanner { b: text.as_bytes() };
    let root = sc.value(0)?;
    let top = sc.members(root)?;
    // The file's own indent unit: the first member's, else two spaces.
    let unit = top.first().and_then(|(_, k, _)| sc.indent_of(*k)).filter(|u| !u.is_empty()).unwrap_or_else(|| "  ".to_string());
    let mut inserts: Vec<Insert> = Vec::new();
    match top.iter().find(|(k, _, _)| k == "hooks") {
        None => {
            let mut obj = serde_json::Map::new();
            for (event, entry) in entries {
                let arr = obj.entry(event.to_string()).or_insert_with(|| Value::Array(Vec::new()));
                arr.as_array_mut()?.push(entry.clone());
            }
            let member = format!("\"hooks\": {}", pretty(&Value::Object(obj), &unit, &unit));
            inserts.push(member_insert(&sc, root, &top, &unit, "", &member));
        }
        Some((_, hooks_key, hooks)) => {
            let events = sc.members(*hooks)?;
            let hooks_indent = sc.indent_of(*hooks_key).unwrap_or_default();
            let member_indent = events
                .first()
                .and_then(|(_, k, _)| sc.indent_of(*k))
                .unwrap_or_else(|| format!("{hooks_indent}{unit}"));
            // New events, appended in one insertion after the last member.
            let mut new_members: Vec<String> = Vec::new();
            for (event, entry) in entries {
                match events.iter().find(|(k, _, _)| k == event) {
                    Some((_, ekey, arr)) => {
                        let elems = sc.elements(*arr)?;
                        let event_indent = sc.indent_of(*ekey).unwrap_or_else(|| member_indent.clone());
                        let elem_indent = elems
                            .first()
                            .and_then(|e| sc.indent_of(e.start))
                            .unwrap_or_else(|| format!("{event_indent}{unit}"));
                        let body = pretty(entry, &elem_indent, &unit);
                        inserts.push(match elems.last() {
                            Some(last) => Insert { at: last.end, text: format!(",\n{elem_indent}{body}") },
                            None => Insert { at: arr.start + 1, text: format!("\n{elem_indent}{body}\n{event_indent}") },
                        });
                    }
                    None => {
                        let value = Value::Array(vec![entry.clone()]);
                        new_members.push(format!("\"{event}\": {}", pretty(&value, &member_indent, &unit)));
                    }
                }
            }
            if !new_members.is_empty() {
                let joined = new_members.join(&format!(",\n{member_indent}"));
                inserts.push(member_insert(&sc, *hooks, &events, &unit, &hooks_indent, &joined));
            }
        }
    }
    // Applied from the end, so earlier offsets stay true.
    inserts.sort_by_key(|i| std::cmp::Reverse(i.at));
    let mut out = text.to_string();
    for ins in inserts {
        out.insert_str(ins.at, &ins.text);
    }
    // The proof: the original, with exactly these entries appended, and nothing else changed.
    let mut expected = original;
    let hooks = expected.as_object_mut()?.entry("hooks").or_insert_with(|| Value::Object(serde_json::Map::new()));
    for (event, entry) in entries {
        let arr = hooks.as_object_mut()?.entry(event.to_string()).or_insert_with(|| Value::Array(Vec::new()));
        arr.as_array_mut()?.push(entry.clone());
    }
    let got: Value = serde_json::from_str(&out).ok()?;
    (got == expected).then_some(out)
}

/// An insertion of `member` (`"key": value`) into the object `obj` whose members are `members`: after the last one, or
/// as the only one.
fn member_insert(sc: &Scanner<'_>, obj: Span, members: &[(String, usize, Span)], unit: &str, obj_indent: &str, member: &str) -> Insert {
    match members.last() {
        Some((_, key, last)) => {
            let indent = sc.indent_of(*key).unwrap_or_else(|| format!("{obj_indent}{unit}"));
            Insert { at: last.end, text: format!(",\n{indent}{member}") }
        }
        None => Insert { at: obj.start + 1, text: format!("\n{obj_indent}{unit}{member}\n{obj_indent}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(cmd: &str) -> Value {
        json!({ "hooks": [ { "type": "command", "command": cmd } ] })
    }

    /// A file as Claude Code writes it, keys in its own order, the user's own hooks in it: one new event is added and
    /// every byte before and after the insertion is the original's.
    #[test]
    fn a_new_event_leaves_the_rest_byte_for_byte() {
        let text = "{\n  \"permissions\": {\n    \"allow\": [\"Bash(git status)\"]\n  },\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"python ~/.claude/hooks/lint-guard.py\"\n          }\n        ]\n      }\n    ],\n    \"SessionStart\": [\n      {\n        \"hooks\": [ { \"type\": \"command\", \"command\": \"base hook session-start\" } ]\n      }\n    ]\n  },\n  \"model\": \"opus\"\n}\n";
        let out = add_hook_entries(text, &[("SessionEnd", entry("base hook session-end"))]).expect("added");
        let at = out.find(",\n    \"SessionEnd\"").expect("inserted after the last event");
        assert_eq!(&out[..at], &text[..at], "everything before it unchanged");
        let tail = &text[at..];
        assert!(out.ends_with(tail), "everything after it unchanged");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], "base hook session-end");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "python ~/.claude/hooks/lint-guard.py");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn an_existing_event_gets_the_entry_appended() {
        let text = "{\"hooks\":{\"SessionEnd\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"mine\"}]}]}}";
        let out = add_hook_entries(text, &[("SessionEnd", entry("base hook session-end"))]).expect("added");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["SessionEnd"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], "mine");
        assert!(out.starts_with("{\"hooks\":{\"SessionEnd\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"mine\"}]}"));
    }

    #[test]
    fn no_hooks_key_and_an_empty_file() {
        for text in ["{}\n", "{\n  \"model\": \"opus\"\n}\n", "{ \"hooks\": {} }", "{\"hooks\":{\"Stop\":[]}}"] {
            let out = add_hook_entries(text, &[("Stop", entry("base hook stop")), ("SessionEnd", entry("base hook session-end"))])
                .unwrap_or_else(|| panic!("added to {text:?}"));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["hooks"]["Stop"].as_array().unwrap().last().unwrap()["hooks"][0]["command"], "base hook stop", "{text:?}");
            assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], "base hook session-end", "{text:?}");
        }
    }

    #[test]
    fn not_an_object_is_refused() {
        assert!(add_hook_entries("[]", &[("Stop", entry("x"))]).is_none());
        assert!(add_hook_entries("{\"hooks\": []}", &[("Stop", entry("x"))]).is_none(), "hooks that is not an object");
        assert!(add_hook_entries("not json", &[("Stop", entry("x"))]).is_none());
    }
}
