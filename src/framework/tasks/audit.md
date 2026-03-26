<purpose>
Deep workspace optimization. Dynamically generate audit phases from the workspace manifest, run each area's configured audit strategy, and execute operator-approved changes.
</purpose>

<user-story>
As an AI builder, I want a thorough workspace audit that adapts to my workspace structure, so that every area gets properly reviewed regardless of how complex my setup is.
</user-story>

<when-to-use>
- Quarterly or after major workspace shifts
- When user says "base audit", "deep clean", "optimize workspace"
- Entry point routes here via /base:audit
</when-to-use>

<steps>

<step name="generate_phases" priority="first">
Read workspace manifest and generate audit phases dynamically.

1. Read `.base/workspace.json`
2. For each area, create an audit phase using its configured strategy
3. Present phase list: "Audit will cover {N} phases: {list with strategies}"
4. Create task tracking for each phase

**Wait for operator confirmation. Allow them to skip or reorder phases.**
</step>

<step name="execute_phases">
Run each phase using its configured audit strategy.

For each phase:
1. Announce: "Phase {N}: {area-name} ({strategy})"
2. Execute the strategy (reference frameworks/audit-strategies.md)
3. Present findings
4. Collect operator decisions (keep/archive/delete/move)
5. Execute approved changes
6. Mark phase complete

Strategies are documented in `@frameworks/audit-strategies.md`.
</step>

<step name="record_audit">
Record the audit results.

1. Update `.base/data/state.json`
2. Write audit record to `.base/audits/{YYYY-MM-DD}.md`
3. Log to `.base/ROADMAP.md`
4. Report final summary: phases completed, items changed, new drift score
</step>

</steps>

<output>
Complete workspace audit with dynamic phases. All areas reviewed, changes executed, audit recorded.
</output>

<acceptance-criteria>
- [ ] Phases generated dynamically from manifest (not hardcoded)
- [ ] Each area audited using its configured strategy
- [ ] Operator approved all changes before execution
- [ ] Audit record written to audits/ directory
- [ ] state.json updated
- [ ] ROADMAP.md updated with audit entry
</acceptance-criteria>
