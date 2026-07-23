# Implementation Plan

1. Introduce a one-record-at-a-time journal reader.
2. Rebuild compatibility scanning on the streaming primitive.
3. Add bounded deterministic segment rotation and directory validation.
4. Add streaming segmented replay with sequence enforcement.
5. Add the explicit checksummed `ReplayState` checkpoint codec.
6. Add operational CLI and failure/integration tests.
7. Update architecture and complete verification.
