# Implementation Plan

1. Define fixed-point price, quantity, and money types with checked conservative
   arithmetic in `common-types`.
2. Define and manually encode the versioned event envelope in `event-schema`.
3. Define the journal file and record formats in `market-recorder`.
4. Implement scanning, corruption detection, and incomplete-tail recovery.
5. Add unit, property, and recovery tests.
6. Model capital reservation safety in TLA+.
7. Enforce formatting, linting, and tests in CI.

