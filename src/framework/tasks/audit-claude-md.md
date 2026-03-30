<purpose>
Audit an existing CLAUDE.md against the CLAUDE.md Strategy framework, then interactively rewrite it with user approval at each stage. Detects CARL installation and routes operational rules accordingly.
</purpose>

<user-story>
As an AI builder, I want my CLAUDE.md audited against a proven strategy so I get a compliant, lean configuration file — with operational rules properly routed to CARL or preserved as an artifact for later.
</user-story>

<when-to-use>
- During /base:scaffold (optional step)
- When user says "audit my claude.md", "improve my claude.md", "rewrite my claude.md"
- Entry point: /base:audit-claude-md
</when-to-use>

<context-files>
@~/.claude/base-framework/frameworks/claudemd-strategy.md
@~/.claude/base-framework/templates/claudemd-template.md
</context-files>

<steps>

<step name="load_strategy" priority="first">
Load the CLAUDE.md Strategy framework and template.

1. Read `@~/.claude/base-framework/frameworks/claudemd-strategy.md` — this is the source of truth
2. Read `@~/.claude/base-framework/templates/claudemd-template.md` — this is the structural reference
3. Internalize: five-section model (What/Why/Who/Where/How), NEVER pattern, line budget, audit criteria

You MUST understand the full strategy before reading the user's file. The strategy defines what "correct" looks like.
</step>

<step name="read_and_catalog">
Read the user's existing CLAUDE.md and catalog every piece of content.

1. Read `CLAUDE.md` from workspace root
2. If no CLAUDE.md exists → skip to `generate_fresh` step
3. For every section, paragraph, rule, table, and reference in the file, classify each as:
   - **KEEP** — belongs in CLAUDE.md per the strategy (identity, structure, constitutional rules)
   - **REMOVE** — doesn't belong (volatile data, task lists, state references, redundant sections)
   - **RESTRUCTURE** — right content, wrong location or format (e.g., rule using "always" instead of NEVER pattern, operational content in wrong section)
   - **CARL_CANDIDATE** — operational rule or domain-specific behavior that belongs in a rules engine, not CLAUDE.md

4. Count total lines. Note if over 100-line budget.
</step>

<step name="present_audit">
Present the full audit to the user. This is INTERACTIVE — do not proceed without approval.

Present a structured report:

**Section Order Compliance:**
- Current order vs required order (What → Why → Who → Where → How)
- Orphan sections (content outside the five-section model)

**Content Classification:**
For each piece of existing content, show:
```
[KEEP] "Business context section" → stays in Who
[REMOVE] "Active Work section" → volatile, belongs in state management
[RESTRUCTURE] "LSP rule" → move to How/Rules, convert to NEVER pattern
[CARL_CANDIDATE] "When writing tests, always..." → operational rule, not identity
```

**Line Budget:**
- Current: {N} lines
- Target: under 100
- Reduction plan: what removal/restructuring achieves

**Missing Content:**
- Sections required by strategy that don't exist yet

Ask: **"Does this audit look right? Any items you want to reclassify before I proceed?"**

Wait for user response. Adjust classifications based on their feedback.
</step>

<step name="detect_carl">
Check for CARL installation to determine rule routing.

1. Check for `.carl/manifest` in workspace root (workspace-level CARL)
2. Check for `~/.carl/manifest` (global-level CARL)
3. If CARL found:
   - Report: "CARL detected at {location}. Operational rules will be proposed as CARL domain rules."
   - Note which existing CARL domains overlap with CARL_CANDIDATE items
4. If CARL not found:
   - Report: "No CARL installation detected."
   - Offer: "I can: (a) install CARL now and set up domains, or (b) save operational rules as an artifact in `.base/artifacts/` for later CARL setup"
   - If user picks (b): rules go to `.base/artifacts/claudemd-audit-rules.md` (create `.base/artifacts/` if needed)
   - If user picks (a): guide CARL installation, then route rules to domains

Wait for user decision before proceeding.
</step>

<step name="propose_rewrite">
Build the new CLAUDE.md section by section, presenting each for approval.

For EACH section (What, Why, Who, Where, How):

1. **Show the proposed content** for that section
2. **Show what changed** vs the original (additions, removals, restructuring)
3. **Ask for approval**: "Accept this section? Or modify?"
4. If user modifies → incorporate changes
5. If user accepts → lock section, move to next

**Section-specific guidance:**

**What:** One-liner. Rarely needs changes unless missing entirely.

**Why:** Philosophy/identity. Pull from existing philosophy content. Strip operational routing details that belong in How.

**Who:** Business context. Preserve existing content. Trim if bloated. Ensure it answers "what does this person's business look like?"

**Where:** Scan actual filesystem with `ls` to verify tree accuracy. Update tree to match reality. Remove subdirectories that don't add meaning. Collapse verbose trees to stay within budget.

**How:** Assemble from:
- Systems table (compact, one row per system)
- Git strategy table (from existing or detected .gitignore)
- Rules (NEVER pattern only — constitutional rules stay here, operational rules route to CARL/artifact)
- Quick reference (common actions → instructions)
</step>

<step name="route_carl_candidates">
Handle operational rules that were classified as CARL_CANDIDATE.

**If CARL is installed:**
1. Group candidates by likely CARL domain (DEVELOPMENT, CONTENT, CLIENTS, etc.)
2. Present: "These rules are proposed for CARL domain `{domain}`:"
3. Show each rule in NEVER pattern format
4. Ask: "Approve these for CARL? Modify? Skip?"
5. For approved rules: provide the exact CARL command to add them (do NOT execute without explicit permission)

**If CARL artifact path:**
1. Write all CARL_CANDIDATE rules to `.base/artifacts/claudemd-audit-rules.md`
2. Format: group by proposed domain, NEVER pattern, include rationale
3. Tell user: "Rules saved to `.base/artifacts/claudemd-audit-rules.md`. When you're ready to set up CARL, run this file through Claude to create the domains."
</step>

<step name="write_and_finalize">
Write the approved CLAUDE.md.

1. Assemble all approved sections into final document
2. Verify line count (warn if over 100)
3. Write to `CLAUDE.base.md` in workspace root (NEVER overwrite CLAUDE.md directly)
4. Present final diff summary: sections added, removed, restructured, rules routed

Tell user:
- "Review `CLAUDE.base.md`. To adopt it: `mv CLAUDE.base.md CLAUDE.md`"
- "Your original CLAUDE.md is untouched."
- If CARL candidates were routed: "Operational rules are in {location}."
</step>

</steps>

<output>
- `CLAUDE.base.md` — strategy-compliant CLAUDE.md ready for adoption
- CARL domain rules (if CARL installed) or `.base/artifacts/claudemd-audit-rules.md` (if not)
- Original CLAUDE.md untouched
</output>

<acceptance-criteria>
- [ ] Strategy framework loaded and understood before audit begins
- [ ] Every line of existing CLAUDE.md classified (KEEP/REMOVE/RESTRUCTURE/CARL_CANDIDATE)
- [ ] Full audit presented to user with approval gate before rewriting
- [ ] CARL installation detected and rule routing decided with user
- [ ] Each section proposed individually with user approval
- [ ] All rules use NEVER pattern
- [ ] Final output under 100 lines
- [ ] Operational rules routed to CARL or saved as artifact
- [ ] Original CLAUDE.md never modified
- [ ] User informed of how to adopt and next steps
</acceptance-criteria>
