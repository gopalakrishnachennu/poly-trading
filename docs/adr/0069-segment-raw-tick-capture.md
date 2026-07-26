# ADR 0069: Segment raw tick capture before research export

- Status: Accepted
- Date: 2026-07-25

## Decision

The credentialless public CLOB and reference recorders expose an explicit
segmented-journal mode. The managed launcher chooses that mode for a new,
explicitly started capture session. Segments use the recorder's validated
256 MiB / 1,000,000 record limits.

Existing single-file journals remain immutable legacy inputs. They are not
renamed, migrated, truncated, deleted, or exported automatically.

## Consequences

Completed segments give research export a bounded, immutable input boundary
and prevent one continuously growing capture file from becoming the only
operational storage unit. The active segment is still not an immutable
research input: a later exporter must verify its exact durable prefix before
using it. This change is public, read-only data capture only and creates no
execution authority.
