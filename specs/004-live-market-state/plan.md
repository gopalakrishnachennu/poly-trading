# Implementation Plan

1. Add an optional bounded envelope sink to public capture.
2. Preserve journal-first ordering for market and epoch events.
3. Implement a deterministic actor health core around `ReplayState`.
4. Add a Tokio single-writer runtime with watch-only snapshots.
5. Build a read-only live-state recorder executable.
6. Prove failure behavior and live/offline digest equivalence.
7. Update architecture and complete repository verification.
