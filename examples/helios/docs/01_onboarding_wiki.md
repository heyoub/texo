---
title: Helios Engineering Onboarding
last_touched: "whenever someone remembers"
owner: it-was-alice-i-think
---

# Welcome to Helios

Helios is our multi-tenant data plane. This wiki is the source of truth for new
engineers. (Narrator: it was not the source of truth.)

If you are reading this, someone added you to the `#helios` channel and then
immediately went on call. Good luck.

## The basics

Helios ingests tenant events, fans them into per-tenant shards, and compiles a
nightly rollup. Most of this still works the way you'd expect.

## Shipping your first change

Deploys happen on Friday. Get your PR reviewed by Thursday EOD or it waits a week.

Alice owns release approval. Ping her before you merge anything to `main`.

Read the old checklist before shipping. It lives in the wiki somewhere.

## Where stuff lives

```yaml
# do NOT treat this block as claims — it is config, inside a fence
deploy:
  day: friday
  approver: alice
```

The platform uses Postgres for storage. Connection strings are in Vault.

That's it. Ask in the channel if you're stuck.
