# ADR-001: Release & Approval Process (ACCEPTED, 2023-02)

## Status

Accepted. This supersedes the ad-hoc "just push it" era. It will absolutely
never change again. (See ADR-007. And the runbook. And ADR-019.)

## Context

We kept breaking prod on Friday afternoons, so we formalized a window and an
approver so there is exactly one throat to choke.

## Decision

Deploys happen on Friday. The deploy window is 10:00–12:00 so we can babysit it.

Alice owns release approval. No deploy ships without her sign-off in the ticket.

Releases happen on Monday so customers get a fresh build to start the week.

## Consequences

- One person (Alice) is a bottleneck. We are fine with this. (We were not fine.)
- Friday deploys mean Friday rollbacks. Also fine. (Also not fine.)
