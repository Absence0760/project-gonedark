---
description: Append a new ADR-style entry to docs/decisions.md and sync related docs.
argument-hint: <the decision and its reasoning, in your own words>
---

Record a new design decision for *Going Dark*.

The user's decision: **$ARGUMENTS**

Do this:

1. Read `docs/decisions.md`, find the highest existing `Dn`, and append a new
   `## D{n+1} — <short title>` entry at the **bottom** (the log is append-only, newest
   last). Follow the existing format exactly:
   - **Decision:** what was chosen, stated plainly.
   - **Why:** the reasoning (this is mandatory — never omit it).
   - **Consequences:** (optional) downstream effects, only if non-trivial.
2. If this decision **resolves an open question**, remove that `Qn` from
   `docs/open-questions.md` (and renumber only if needed — otherwise leave a short note
   that it was resolved in `D{n+1}`).
3. Update any other doc the decision touches (README status/repo-map, architecture,
   platforms, roadmap) so nothing contradicts the new entry.
4. Verify cross-references still resolve. Summarize what you changed.

If `$ARGUMENTS` is too vague to capture faithfully, ask the user one clarifying
question before writing — do not invent rationale.
