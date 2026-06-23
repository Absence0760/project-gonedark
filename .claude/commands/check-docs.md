---
description: Run a consistency sweep over the design corpus (delegates to design-doc-keeper).
allowed-tools: Task, Read, Grep, Glob
---

Sanity-check the *Going Dark* design corpus for drift before sharing or committing.

Launch the `design-doc-keeper` agent to verify: decision-log format and numbering,
open-questions sync, cross-reference/link integrity, README repo-map completeness, and
that nothing contradicts the CLAUDE.md invariants.

Report back what it fixed mechanically and what needs a human decision. Do not make
design changes yourself — only mechanical consistency fixes are allowed.
