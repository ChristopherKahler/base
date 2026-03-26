<purpose>
Structured CARL domain maintenance. Review staged proposals, flag stale rules, audit domain health, and keep CARL lean and accurate.
</purpose>

<user-story>
As an AI builder, I want a guided CARL maintenance session, so that my domain rules stay relevant, staged proposals get decided on, and CARL doesn't become a dumping ground of stale rules.
</user-story>

<when-to-use>
- Monthly (on configured cadence)
- When pulse reports overdue CARL hygiene
- When user says "carl hygiene", "review carl rules", "clean up carl"
- Entry point routes here via /base:carl-hygiene
</when-to-use>

<steps>

<step name="assess" priority="first">
Gather CARL health data and present summary.

1. Read `.base/workspace.json` for `carl_hygiene` config (threshold, max rules, last run)
2. Read `.base/staging.json` for pending proposals (via `carl_get_staged`)
3. For each domain file in `.carl/`:
   - Count rules
   - Parse metadata comments (`# Last reviewed:` lines)
   - Flag rules where last_reviewed is older than `staleness_threshold_days`
   - Flag domains exceeding `max_rules_per_domain`
4. Present summary:
   ```
   CARL Hygiene Assessment
   ━━━━━━━━━━━━━━━━━━━━━━
   Staged proposals: {N} pending
   Domains: {N} total, {N} with stale rules, {N} over max
   Total rules: {N} across all domains
   Last hygiene: {date or "never"}
   ```

**Wait for operator confirmation before proceeding.**
</step>

<step name="review_proposals">
Process each pending staged proposal.

For each pending proposal from `carl_get_staged`:
1. Present:
   ```
   Proposal {id} — {domain}
   Proposed: {date} | Source: {source}
   Rule: "{rule_text}"
   Rationale: {rationale}
   ```
2. Ask: "**Approve**, **Kill**, **Archive**, or **Defer**?"
3. Execute:
   - Approve → `carl_approve_proposal(id)` (writes rule to domain file with metadata)
   - Kill → `carl_kill_proposal(id)`
   - Archive → `carl_archive_proposal(id)`
   - Defer → skip (stays pending for next hygiene)

**After approval:** Add metadata comments above the new rule in the domain file:
```
# Rule added: {today}
# Last reviewed: {today}
# Source: {proposal.source}
{DOMAIN}_RULE_{N}={rule_text}
```

Process one proposal at a time. Wait for response between each.
</step>

<step name="review_stale_rules">
Review rules flagged as stale (last_reviewed > threshold).

For each domain with stale rules:
1. Present domain name and total rule count
2. For each stale rule:
   ```
   [{DOMAIN}] Rule {N} — last reviewed {date} ({days} days ago)
   "{rule_text}"
   ```
3. Ask: "**Keep** (update reviewed date), **Archive**, or **Kill**?"
4. Execute:
   - Keep → Update `# Last reviewed:` comment to today
   - Archive → Remove rule from domain file, add to `.carl/archive/archived-rules.json` with context
   - Kill → Remove rule from domain file (with "Are you sure?" confirmation)

**When archiving:** Read `.carl/archive/archived-rules.json`, append:
```json
{
  "domain": "DOMAIN",
  "rule_number": N,
  "rule_text": "the rule",
  "added": "original-added-date",
  "archived": "today",
  "reason": "operator's reason"
}
```

Then renumber remaining rules in the domain file to close gaps.

Process one domain at a time.
</step>

<step name="review_domains">
Quick domain health check — guided Q&A.

1. List all active domains with rule counts
2. Cross-reference with `.base/data/active.json` current projects:
   - Any active projects missing a domain?
   - Any domains for inactive/completed projects?
3. Check recall phrases:
   - "Do the recall phrases for {domain} still match how you talk about this work?"
4. Check for domains over `max_rules_per_domain`:
   - "Domain {X} has {N} rules (max: {max}). Any candidates to archive or consolidate?"
5. Ask: "Any new domains to create? Any to deactivate?"

**Guided Q&A — don't force changes, just surface questions.**
</step>

<step name="review_decisions">
Quick check on per-domain decision health.

1. List `.carl/decisions/*.json` files with counts
2. Flag domains with 0 decisions (might be missing logging)
3. Flag decisions older than 90 days (might be outdated)
4. Ask: "Any decisions to review or archive?"

**Brief pass — decisions are mostly self-maintaining.**
</step>

<step name="log">
Record the hygiene session.

1. Update `.base/workspace.json` → `carl_hygiene.last_run` to today's date
2. Update `.base/data/state.json` → note CARL hygiene completed
3. Report:
   ```
   CARL Hygiene Complete
   ━━━━━━━━━━━━━━━━━━━━━
   Proposals: {N} processed ({N} approved, {N} killed, {N} archived, {N} deferred)
   Rules reviewed: {N} ({N} kept, {N} archived, {N} killed)
   Domains: {N} active
   Next hygiene due: {date based on cadence}
   ```
</step>

</steps>

<output>
CARL domains reviewed and maintained. Staged proposals decided. Stale rules addressed. Domain health verified. Hygiene session logged to workspace.json.
</output>

<acceptance-criteria>
- [ ] All pending proposals presented and decided (approve/kill/archive/defer)
- [ ] Stale rules flagged and reviewed with operator
- [ ] Domain health check completed (rule counts, recall phrases)
- [ ] Archived rules moved to .carl/archive/archived-rules.json
- [ ] workspace.json carl_hygiene.last_run updated
- [ ] state.json updated with hygiene completion
- [ ] Operator confirmed completion of each step
</acceptance-criteria>
