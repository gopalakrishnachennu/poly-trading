# Implementation Plan

1. Add strict decimal-to-micros parsing to canonical financial types.
2. Decode versioned public payloads into typed fixed-point events.
3. Build a deterministic single-writer book state machine.
4. Replay clean journals with contiguous sequence enforcement.
5. Emit an explicit stable state digest from a read-only CLI.
6. Add failure, golden, and replay-equivalence tests.
7. Update architecture and complete repository verification.
