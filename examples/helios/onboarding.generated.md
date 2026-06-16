# Generated Onboarding

_This document is a projection replayed from the texo claim-chain. It is not source truth._

_Replayed through local store sequence 55._

## Current claims

- **claim_248362aa6b07** (Helios): Helios is our multi-tenant data plane.  
  _source: 01_onboarding_wiki.md:9_
- **claim_e73aedae4a4b** (this wiki): This wiki is the source of truth for new engineers.  
  _source: 01_onboarding_wiki.md:9_
- **claim_257df07845d3** (Helios): Helios ingests tenant events.  
  _source: 01_onboarding_wiki.md:17_
- **claim_c597d0f0f6d4** (Helios): Helios fans tenant events into per-tenant shards.  
  _source: 01_onboarding_wiki.md:17_
- **claim_95015fbbf798** (Helios): Helios compiles a nightly rollup.  
  _source: 01_onboarding_wiki.md:17_
- **claim_0a2bb06835d1** (connection strings): Connection strings are in Vault.  
  _source: 01_onboarding_wiki.md:37_
- **claim_47a0d9d6abae** (deploy window): The deploy window is 10:00–12:00.  
  _source: 02_adr_001_release_process.md:15_
- **claim_cfa836ac174a** (ADR-007): ADR-007 supersedes the deploy-day decision in ADR-001.  
  _source: 03_adr_007_deploy_window.md:5_
- **claim_b9fa0ddb813d** (Deploys): Deploys moved to Tuesday.  
  _source: 04_release_runbook.md:8_
- **claim_da879fdd38a0** (Helios deploy): Deploys are run with `helios deploy --tenant all` from the bastion.  
  _source: 04_release_runbook.md:14_
- **claim_b0d0e2eb591f** (rollback procedure): To roll back, run `helios rollback` and post in `#incident`.  
  _source: 04_release_runbook.md:24_
- **claim_c22837da62af** (rollup memory limit): The rollup memory limit was bumped.  
  _source: 05_meeting_dump.md:8_
- **claim_6148a65bf27e** (Bob): Bob owns release approval now.  
  _source: 05_meeting_dump.md:11_
- **claim_f649b52adae9** (Alice): Alice is moving to the data-platform team.  
  _source: 05_meeting_dump.md:11_
- **claim_b5c8241aa790** (Friday freeze): The Friday freeze is gone.  
  _source: 05_meeting_dump.md:15_
- **claim_c692bdaeebd8** (Partners): Partners schedule their integration tests for the weekend.  
  _source: 06_rogue_partner_runbook.md:8_
- **claim_b078220ce896** (Partner escalations): Partner escalations go to #partners-oncall.  
  _source: 06_rogue_partner_runbook.md:16_
- **claim_5030e093e737** (ADR-019): ADR-019 supersedes the Postgres-as-event-store assumption.  
  _source: 07_storage_adr.md:5_
- **claim_0d6f1980a02a** (Postgres): Postgres stays as the relational metadata store for tenant config.  
  _source: 07_storage_adr.md:10_
- **claim_0b4b01151c2d** (event table): The home-grown event table was replaced with BatPak's content-addressed log.  
  _source: 07_storage_adr.md:13_
- **claim_b1362d7fa8e5** (events table): The old `events` table is deprecated and read-only.  
  _source: 07_storage_adr.md:18_

## Stale claims (do not trust)

- claim_11fdfabeb326: "No deploy ships without Alice's sign-off in the ticket." superseded by claim_6148a65bf27e
- claim_147b8cdcc3f5: "The deploy day was changed, but the release cadence was not changed." superseded by claim_b9fa0ddb813d
- claim_47ad6f4cd98c: "Deploys happen on Friday." superseded by claim_b5c8241aa790
- claim_65405a8beed6: "Alice owns release approval." superseded by claim_6148a65bf27e
- claim_84ebb94ec3d0: "Alice owns release approval." superseded by claim_6148a65bf27e
- claim_aae976fb3395: "Deploys moved to Wednesday." superseded by claim_b9fa0ddb813d
- claim_af7c05c36559: "The platform uses Postgres for storage." superseded by claim_5030e093e737
- claim_bda7fd8df546: "Approval goes through the existing owner." superseded by claim_6148a65bf27e
- claim_be9435f8c442: "Deploys happen on Friday." superseded by claim_b5c8241aa790
- claim_d152ae4ff246: "The platform uses BatPak for append-only event storage now." superseded by claim_0b4b01151c2d

## Conflicts (unresolved — both claimed, neither wins)

- "Releases happen on Monday." (claim_e9943d9af690) vs "Releases go out on Friday." (claim_bec5d89765f5)
