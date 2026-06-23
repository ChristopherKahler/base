# Query-Triggered Injection — Test Guide

## What to look for

When you type a prompt containing an AUDIENCE keyword (`icp`, `audience`, `target customer`), the system should inject a `<base-query>` block into your context. Check DEVMODE output:

**Working:**
```
LOADED DOMAINS:
  [AUDIENCE] keyword (0 rules)
```
Plus a `<base-query name="icp-context" domain="AUDIENCE">` block in the injected context with ICP notes.

**Not working:**
```
AVAILABLE (not loaded):
  AUDIENCE (icp, audience, ...)
```
AUDIENCE stays in AVAILABLE = keyword matched but output was empty so domain got skipped.

---

## Test prompts

Copy-paste ONE of these as your full prompt in a fresh session:

### Test 1: Single keyword
```
icp
```

### Test 2: Phrase match
```
who is my target audience
```

### Test 3: Embedded keyword
```
I need to write copy for my ideal customer
```

### Test 4: Combined with task
```
write me a hook for a reel — keep the icp in mind
```

---

## What the injection should look like

```xml
<base-query name="icp-context" domain="AUDIENCE">
- Target audience: Solo operators, small-team founders, indie builders...
- Pain point: AI was supposed to buy back their time...
- Desire: Work should serve life, not consume it...
- Psychographic: Builder identity. Ships things...
- Language bank: 'quit playing with prompts'...
- Stage: Has tried AI tools. Got initial results...
- Competitor gap: Most Claude Code content is beginner...
</base-query>
```

---

## Pre-flight checklist

Run these from any workspace before testing:

```bash
# 1. Verify binary is latest (should be 0.1.9)
base --version

# 2. Verify query file exists
cat ~/.base-gbl/queries/icp-context.sparql

# 3. Verify AUDIENCE domain is in domains.toml
grep -A5 "AUDIENCE" ~/.base-gbl/domains.toml

# 4. Verify ICP data is in the global graph
grep "domain/audience" ~/.base-gbl/.base/graph.nq | wc -l
# Should be 7+ lines

# 5. Verify query fires via CLI
echo '{"prompt": "icp"}' | base hook user-prompt-submit 2>/dev/null | grep "base-query"
# Should print: <base-query name="icp-context" domain="AUDIENCE">

# 6. Clear session state (fresh start)
rm -f ~/.base-gbl/.base/.session
rm -f .base/.session 2>/dev/null
```

If step 5 returns nothing, the query isn't reaching the data. Check:
- `grep "Note" ~/.base-gbl/.base/graph.nq | wc -l` — should be 7+
- `grep "noteText" ~/.base-gbl/.base/graph.nq | head -3` — should show ICP text

---

## If it still doesn't work

Capture the full hook output for diagnosis:
```bash
rm -f .base/.session 2>/dev/null
echo '{"prompt": "icp"}' | base hook user-prompt-submit > /tmp/hook-output.txt 2>/tmp/hook-stderr.txt
cat /tmp/hook-output.txt
cat /tmp/hook-stderr.txt
```

Then paste both files into the session.
