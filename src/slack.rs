//! The Slack rail: `base slack post|read|channels`.
//!
//! Chris, 2026-09-02: "make sure that this Slack channel is accessible
//! everywhere in my machine. I need my Claude to easily be able to drop a
//! message there at any moment, without questions." A session has `base` on
//! its PATH on every OS, so the rail is a subcommand, not an MCP, not a daemon:
//! one bot token in the secret store (`SLACK_BOT_TOKEN`, `base secret set`),
//! one HTTPS call per message. Messages post as the workspace's own app.
//!
//! `--to` accepts a channel id (`C…`, `G…`, `D…`), a `#name` (resolved through
//! conversations.list, so the bot needs channels:read / groups:read), or a
//! message permalink (`https://x.slack.com/archives/C…/p1788316929791049`),
//! which posts a reply in that message's thread. Scopes the app needs:
//! chat:write, chat:write.public, channels:read, channels:history, groups:read,
//! groups:history; private channels also need the app invited.

use anyhow::{anyhow, bail, Context, Result};

const API: &str = "https://slack.com/api";
const TIMEOUT_SECS: u64 = 20;
pub const TOKEN_KEY: &str = "SLACK_BOT_TOKEN";

/// Where a message goes: a conversation id, optionally inside a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub channel: String,
    pub thread_ts: Option<String>,
}

/// What `--to` may say, before any API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    /// A conversation id: C…, G… (private), D… (DM).
    Id(String),
    /// A `#channel-name`, resolved by listing.
    Name(String),
    /// A message permalink: channel id + the message's ts, i.e. a thread.
    Permalink { channel: String, ts: String },
}

/// Parse `--to`. Pure. A permalink's `p1788316929791049` is the ts with the
/// dot removed: the last six digits are the fraction.
pub fn parse_addr(to: &str) -> Result<Addr> {
    let t = to.trim();
    if let Some(rest) = t.strip_prefix('#') {
        if rest.is_empty() {
            bail!("empty channel name");
        }
        return Ok(Addr::Name(rest.to_string()));
    }
    if t.contains("/archives/") {
        let after = t.split("/archives/").nth(1).ok_or_else(|| anyhow!("bad permalink: {t}"))?;
        let mut parts = after.split('/').filter(|s| !s.is_empty());
        let channel = parts.next().ok_or_else(|| anyhow!("bad permalink: {t}"))?.to_string();
        let p = parts
            .next()
            .and_then(|s| s.split('?').next())
            .ok_or_else(|| anyhow!("permalink has no message: {t}"))?;
        let digits = p.strip_prefix('p').unwrap_or(p);
        if digits.len() < 7 || !digits.chars().all(|c| c.is_ascii_digit()) {
            bail!("bad message id in permalink: {p}");
        }
        let (secs, frac) = digits.split_at(digits.len() - 6);
        return Ok(Addr::Permalink { channel, ts: format!("{secs}.{frac}") });
    }
    let looks_like_id = t.len() >= 9
        && matches!(t.chars().next(), Some('C' | 'G' | 'D'))
        && t.chars().all(|c| c.is_ascii_alphanumeric());
    if looks_like_id {
        return Ok(Addr::Id(t.to_string()));
    }
    // A bare name without '#': treat as a channel name.
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Ok(Addr::Name(t.to_string()));
    }
    bail!("cannot read a Slack target from {t:?} — use C… id, #name, or a message permalink")
}

/// The bot token: the environment first (a CI or a rail daemon), then the
/// secret store.
pub fn token() -> Result<String> {
    if let Ok(v) = std::env::var(TOKEN_KEY)
        && !v.trim().is_empty()
    {
        return Ok(v.trim().to_string());
    }
    crate::secret::get(TOKEN_KEY)?
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "no {TOKEN_KEY}. Create a bot token for your Slack app (api.slack.com → OAuth & \
                 Permissions → Bot User OAuth Token, scopes chat:write chat:write.public \
                 channels:read channels:history groups:read groups:history) and store it with \
                 `base secret set {TOKEN_KEY}`"
            )
        })
}

fn call(method: &str, token: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let resp = ureq::post(&format!("{API}/{method}"))
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json; charset=utf-8")
        .send_json(body)
        .with_context(|| format!("slack {method}: request failed"))?;
    let v: serde_json::Value = resp.into_json().with_context(|| format!("slack {method}: bad JSON"))?;
    if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error");
        let hint = match err {
            "not_in_channel" => " — invite the app to that channel (/invite @<app>)",
            "channel_not_found" => " — wrong id, or a private channel the app is not in",
            "missing_scope" => " — add the scope in api.slack.com and reinstall the app",
            "invalid_auth" | "token_revoked" | "account_inactive" => " — the token is dead; make a new one and `base secret set SLACK_BOT_TOKEN`",
            _ => "",
        };
        bail!("slack {method}: {err}{hint}");
    }
    Ok(v)
}

/// Resolve `#name` to an id by listing conversations the app can see.
fn resolve_name(token: &str, name: &str) -> Result<String> {
    let mut cursor = String::new();
    for _ in 0..20 {
        let mut body = serde_json::json!({
            "types": "public_channel,private_channel",
            "exclude_archived": true,
            "limit": 200,
        });
        if !cursor.is_empty() {
            body["cursor"] = serde_json::Value::String(cursor.clone());
        }
        let v = call("conversations.list", token, body)?;
        if let Some(chs) = v.get("channels").and_then(|c| c.as_array()) {
            for ch in chs {
                if ch.get("name").and_then(|n| n.as_str()) == Some(name)
                    && let Some(id) = ch.get("id").and_then(|i| i.as_str())
                {
                    return Ok(id.to_string());
                }
            }
        }
        cursor = v
            .pointer("/response_metadata/next_cursor")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if cursor.is_empty() {
            break;
        }
    }
    bail!("no channel named #{name} visible to the app (private? invite the app first)")
}

/// Turn `--to` (+ optional `--thread`) into a concrete target.
pub fn resolve(token: &str, to: &str, thread: Option<&str>) -> Result<Target> {
    let (channel, ts) = match parse_addr(to)? {
        Addr::Id(id) => (id, None),
        Addr::Name(name) => (resolve_name(token, &name)?, None),
        Addr::Permalink { channel, ts } => (channel, Some(ts)),
    };
    Ok(Target { channel, thread_ts: thread.map(str::to_string).or(ts) })
}

/// Post `text`; returns the message permalink.
pub fn post(token: &str, target: &Target, text: &str) -> Result<String> {
    let mut body = serde_json::json!({ "channel": target.channel, "text": text });
    if let Some(ts) = &target.thread_ts {
        body["thread_ts"] = serde_json::Value::String(ts.clone());
    }
    let v = call("chat.postMessage", token, body)?;
    let ts = v.get("ts").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let channel = v.get("channel").and_then(|c| c.as_str()).unwrap_or(&target.channel).to_string();
    let link = call("chat.getPermalink", token, serde_json::json!({ "channel": channel, "message_ts": ts }))
        .ok()
        .and_then(|p| p.get("permalink").and_then(|l| l.as_str()).map(String::from))
        .unwrap_or_else(|| format!("{channel} ts={ts}"));
    Ok(link)
}

/// One line per message: `ts  user  text`, oldest first. A thread target reads
/// the thread; a channel target reads recent history.
pub fn read(token: &str, target: &Target, limit: usize) -> Result<Vec<String>> {
    let (method, mut body) = match &target.thread_ts {
        Some(ts) => ("conversations.replies", serde_json::json!({ "channel": target.channel, "ts": ts })),
        None => ("conversations.history", serde_json::json!({ "channel": target.channel })),
    };
    body["limit"] = serde_json::Value::from(limit.clamp(1, 200) as u64);
    let v = call(method, token, body)?;
    let mut lines: Vec<String> = v
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|msgs| {
            msgs.iter()
                .map(|m| {
                    let ts = m.get("ts").and_then(|t| t.as_str()).unwrap_or("");
                    let who = m
                        .get("user")
                        .or_else(|| m.get("username"))
                        .or_else(|| m.get("bot_id"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("?");
                    let text = m.get("text").and_then(|t| t.as_str()).unwrap_or("").replace('\n', "\n    ");
                    format!("{ts}  {who}  {text}")
                })
                .collect()
        })
        .unwrap_or_default();
    if method == "conversations.history" {
        lines.reverse(); // history is newest-first; read oldest-first like a transcript
    }
    Ok(lines)
}

/// `id  #name  (private)` for every conversation the app can see.
pub fn channels(token: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..20 {
        let mut body = serde_json::json!({ "types": "public_channel,private_channel", "exclude_archived": true, "limit": 200 });
        if !cursor.is_empty() {
            body["cursor"] = serde_json::Value::String(cursor.clone());
        }
        let v = call("conversations.list", token, body)?;
        for ch in v.get("channels").and_then(|c| c.as_array()).into_iter().flatten() {
            let id = ch.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            let name = ch.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let private = ch.get("is_private").and_then(|p| p.as_bool()).unwrap_or(false);
            let member = ch.get("is_member").and_then(|p| p.as_bool()).unwrap_or(false);
            out.push(format!("{id}  #{name}{}{}", if private { "  (private)" } else { "" }, if member { "  (app is a member)" } else { "" }));
        }
        cursor = v.pointer("/response_metadata/next_cursor").and_then(|c| c.as_str()).unwrap_or("").to_string();
        if cursor.is_empty() {
            break;
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_reads_ids_names_and_permalinks() {
        assert_eq!(parse_addr("C0BRVHJARSS").unwrap(), Addr::Id("C0BRVHJARSS".into()));
        assert_eq!(parse_addr("#base-issues").unwrap(), Addr::Name("base-issues".into()));
        assert_eq!(parse_addr("base-issues").unwrap(), Addr::Name("base-issues".into()));
        assert_eq!(
            parse_addr("https://chrisaisystems.slack.com/archives/C0BRVHJARSS/p1788316929791049").unwrap(),
            Addr::Permalink { channel: "C0BRVHJARSS".into(), ts: "1788316929.791049".into() }
        );
        // a thread permalink carries the parent in the query string; the message itself is the target
        assert_eq!(
            parse_addr("https://x.slack.com/archives/D06SW6CJ7PU/p1788347076488979?thread_ts=1788347000.1&cid=D06SW6CJ7PU").unwrap(),
            Addr::Permalink { channel: "D06SW6CJ7PU".into(), ts: "1788347076.488979".into() }
        );
        assert!(parse_addr("").is_err());
        assert!(parse_addr("https://x.slack.com/archives/C1/pabc").is_err());
    }
}
