# ADR-019: Storage Engine (ACCEPTED, 2024-06)

## Status

Accepted. Supersedes the Postgres-as-event-store assumption baked into the
onboarding wiki and half the codebase.

## Decision

The platform uses BatPak for append-only event storage now. Postgres stays only
as the relational metadata store for tenant config.

We replaced the home-grown event table with BatPak's content-addressed log
because we kept corrupting the sequence column under load.

## Migration

Dual-write is done. The old `events` table is deprecated and read-only.
