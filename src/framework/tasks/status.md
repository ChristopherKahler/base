<purpose>
Quick one-liner workspace health check. No conversation, just the numbers.
</purpose>

<user-story>
As an AI builder, I want a fast health check I can glance at, so that I know if anything needs attention without a full briefing.
</user-story>

<when-to-use>
- When user wants a quick check without full pulse
- Entry point routes here via /base:status
</when-to-use>

<steps>

<step name="quick_check" priority="first">
Read state and output one-liner.

1. Read `.base/data/state.json`
2. Calculate current drift score from timestamps
3. Count overdue areas and past-due backlog items
4. Output single line: "BASE: Drift {score} | {N} areas overdue | {N} backlog items past review-by | Last groom: {date}"
</step>

</steps>

<output>
Single-line health summary. No conversation.
</output>

<acceptance-criteria>
- [ ] Output is one line
- [ ] Drift score is current (not cached)
- [ ] Overdue counts are accurate
</acceptance-criteria>
