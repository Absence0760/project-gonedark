---
name: Bug report
about: Report a defect or unexpected behaviour
title: ""
labels: ["type:bug"]
assignees: []
---

## What happened

<!-- One or two sentences describing the actual behaviour. -->

## What you expected

<!-- One sentence describing the expected behaviour. -->

## How to reproduce

<!-- The minimum set of steps. If it's reproducible headlessly, give the
exact command (`pnpm desktop:sim`, `cargo run -p gonedark-app -- --scene …`)
and the scene/seed — that's far more useful than a description. -->

1.
2.
3.

## Environment

- Commit SHA:
- Platform: <!-- Linux / Windows / Android (device + SoC) -->
- GPU / driver: <!-- e.g. RTX 3070 580.159.04, Adreno 750 -->
- Build profile: <!-- debug / release -->

## Is this a desync?

<!-- If two clients (or two architectures) diverged, this is a determinism bug
and it's high priority — invariant #1. Fill this in; otherwise delete it. -->

- Tick where the checksums first differed:
- Checksum streams / `sim-runner` output:

## Logs / screenshots

```

```

## Severity

- [ ] Blocks me entirely (no workaround)
- [ ] Painful but I have a workaround
- [ ] Mild — quality-of-life
