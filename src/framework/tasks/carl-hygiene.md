<purpose>
Structured CARL domain maintenance. Review staged proposals, flag stale rules, audit domain health, and keep CARL lean and accurate. All operations target carl.json as the single source of truth.
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
2. Use `carl_v2_list_domains` to get all domains with rule/decision counts and state
3. Use `carl_v2_get_staged` to check for pending staging proposals
4. For each active domain from `carl_v2_list_domains`:
   - Note rule count and decision count
   - Use `carl_v2_get_domain(domain)` to inspect rule `last_reviewed` fields
   - Flag rules where `last_reviewed` is null or older than `staleness_threshold_days`
   - Flag domains exceeding `max_rules_per_domain`
5. Present summary:
   ```
   CARL Hygiene Assessment
   ━━━━━━━━━━━━━━━━━━━━━━
   Staged proposals: {N} pending
   Domains: {N} total ({N} active, {N} inactive), {N} with stale rules, {N} over max
   Total rules: {N} across all domains
   Total decisions: {N} ({N} active, {N} archived)
   Last hygiene: {date or "never"}
   ```

**Wait for operator confirmation before proceeding.**
</step>

<step name="review_proposals">
Process each pending staged proposal.

Use `carl_v2_get_staged` to retrieve all proposals. For each with `status: "pending"`:
1. Present:
   ```
   Proposal {id} — {proposed_domain}
   Proposed: {created_at} | Source: {source_session or "manual"}
   Rule: "{rule_text}"
   Rationale: {rationale}
   ```
2. Ask: "**Approve**, **Kill**, or **Defer**?"
3. Execute:
   - Approve → `carl_v2_approve_proposal(id)` — promotes to domain rule with `source: "staging"`, removes from staging
   - Kill → Read `.carl/carl.json`, remove the proposal entry from the `staging` array, write back
   - Defer → skip (stays pending for next hygiene)

If no pending proposals: "No staged proposals to review." and move to next step.

Process one proposal at a time. Wait for response between each.
</step>

<step name="review_stale_rules">
Review rules flagged as stale (last_reviewed is null or older than threshold).

For each domain with stale rules (identified in assess step):
1. Present domain name and total rule count
2. For each stale rule:
   ```
   [{DOMAIN}] Rule {id} — last reviewed {date or "never"} ({days} days ago)
   "{text}"
   ```
3. Ask: "**Keep** (update reviewed date), or **Kill**?"
4. Execute:
   - Keep → Use `carl_v2_replace_rules(domain, rules)` with updated `last_reviewed` set to today's date for kept rules
   - Kill → Use `carl_v2_remove_rule(domain, rule_id)` (with "Are you sure?" confirmation)

If no stale rules: "All rules are current. No staleness issues." and move to next step.

Process one domain at a time.
</step>

<step name="review_domains">
Quick domain health check — guided Q&A.

1. List all domains (active and inactive) with rule counts from `carl_v2_list_domains`
2. For each active domain:
   - "Do the recall phrases for **{domain}** still match how you talk about this work?"
   - Show current recall keywords for reference
3. Check for domains over `max_rules_per_domain`:
   - "Domain **{X}** has {N} rules (max: {max}). Any candidates to kill or consolidate?"
4. Check for inactive domains: "These domains are inactive: {list}. Reactivate or remove any?"
5. Ask: "Any new domains to create? Any to deactivate?"

**Guided Q&A — don't force changes, just surface questions.**
</step>

<step name="review_decisions">
Quick check on per-domain decision health.

For each domain that has decisions (from `carl_v2_get_domain`):
1. List decisions with date and status
2. Flag decisions older than 90 days (might be outdated)
3. Flag domains with 0 decisions that might benefit from decision logging
4. Ask: "Any decisions to archive?" → use `carl_v2_archive_decision(id)` if yes

**Brief pass — decisions are mostly self-maintaining.**
</step>

<step name="log">
Record the hygiene session.

1. Update `.base/workspace.json` → `carl_hygiene.last_run` to today's date
2. Update `.base/data/state.json` → note CARL hygiene completed with timestamp
3. Report:
   ```
   CARL Hygiene Complete
   ━━━━━━━━━━━━━━━━━━━━━
   Proposals: {N} processed ({N} approved, {N} killed, {N} deferred)
   Rules reviewed: {N} ({N} kept, {N} killed)
   Decisions reviewed: {N} ({N} archived)
   Domains: {N} active, {N} inactive
   Next hygiene due: {date based on cadence}
   ```
</step>

</steps>

<output>
CARL domains reviewed and maintained. Staged proposals decided. Stale rules addressed. Domain health verified. Hygiene session logged to workspace.json.
</output>

<acceptance-criteria>
- [ ] All pending proposals presented and decided (approve/kill/defer)
- [ ] Stale rules flagged and reviewed with operator
- [ ] Domain health check completed (rule counts, recall phrases)
- [ ] workspace.json carl_hygiene.last_run updated
- [ ] state.json updated with hygiene completion
- [ ] Operator confirmed completion of each step
</acceptance-criteria>
