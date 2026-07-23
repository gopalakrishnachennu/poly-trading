---------------- MODULE ShadowAdapterCertification ----------------
EXTENDS Naturals, TLC

VARIABLES contract, fixtureCount, baselineDryRun, denialCount, regionCount,
          operationalObserved, operationalHealthy, failureCount, evaluated,
          certified, authorityGranted, unsafeFailureAction, signing,
          authenticated, submitted, halted

vars == <<contract, fixtureCount, baselineDryRun, denialCount, regionCount,
          operationalObserved, operationalHealthy, failureCount, evaluated,
          certified, authorityGranted, unsafeFailureAction, signing,
          authenticated, submitted, halted>>

Init ==
    /\ contract = FALSE
    /\ fixtureCount = 0
    /\ baselineDryRun = FALSE
    /\ denialCount = 0
    /\ regionCount = 0
    /\ operationalObserved = FALSE
    /\ operationalHealthy = FALSE
    /\ failureCount = 0
    /\ evaluated = FALSE
    /\ certified = FALSE
    /\ authorityGranted = FALSE
    /\ unsafeFailureAction = FALSE
    /\ signing = FALSE
    /\ authenticated = FALSE
    /\ submitted = FALSE
    /\ halted = FALSE

RegisterContract ==
    /\ ~halted
    /\ ~contract
    /\ contract' = TRUE
    /\ UNCHANGED <<fixtureCount, baselineDryRun, denialCount, regionCount,
                    operationalObserved, operationalHealthy, failureCount,
                    evaluated, certified, authorityGranted, unsafeFailureAction,
                    signing, authenticated, submitted, halted>>

RecordFixture ==
    /\ ~halted
    /\ contract
    /\ fixtureCount < 9
    /\ fixtureCount' = fixtureCount + 1
    /\ UNCHANGED <<contract, baselineDryRun, denialCount, regionCount,
                    operationalObserved, operationalHealthy, failureCount,
                    evaluated, certified, authorityGranted, unsafeFailureAction,
                    signing, authenticated, submitted, halted>>

RecordBaseline ==
    /\ ~halted
    /\ contract
    /\ ~baselineDryRun
    /\ baselineDryRun' = TRUE
    /\ UNCHANGED <<contract, fixtureCount, denialCount, regionCount,
                    operationalObserved, operationalHealthy, failureCount,
                    evaluated, certified, authorityGranted, unsafeFailureAction,
                    signing, authenticated, submitted, halted>>

RecordDenial ==
    /\ ~halted
    /\ contract
    /\ denialCount < 4
    /\ denialCount' = denialCount + 1
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, regionCount,
                    operationalObserved, operationalHealthy, failureCount,
                    evaluated, certified, authorityGranted, unsafeFailureAction,
                    signing, authenticated, submitted, halted>>

RecordRegion ==
    /\ ~halted
    /\ contract
    /\ regionCount < 2
    /\ regionCount' = regionCount + 1
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, denialCount,
                    operationalObserved, operationalHealthy, failureCount,
                    evaluated, certified, authorityGranted, unsafeFailureAction,
                    signing, authenticated, submitted, halted>>

ObserveOperational(healthy) ==
    /\ ~halted
    /\ contract
    /\ ~operationalObserved
    /\ operationalObserved' = TRUE
    /\ operationalHealthy' = healthy
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, denialCount,
                    regionCount, failureCount, evaluated, certified,
                    authorityGranted, unsafeFailureAction, signing,
                    authenticated, submitted, halted>>

SimulateFailure ==
    /\ ~halted
    /\ contract
    /\ failureCount < 8
    /\ failureCount' = failureCount + 1
    /\ unsafeFailureAction' = FALSE
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, denialCount,
                    regionCount, operationalObserved, operationalHealthy,
                    evaluated, certified, authorityGranted, signing,
                    authenticated, submitted, halted>>

Evaluate ==
    /\ ~halted
    /\ ~evaluated
    /\ evaluated' = TRUE
    /\ certified' = contract /\ fixtureCount = 9 /\ baselineDryRun
                     /\ denialCount = 4 /\ regionCount = 2
                     /\ operationalObserved /\ operationalHealthy
                     /\ failureCount = 8
    /\ authorityGranted' = FALSE
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, denialCount,
                    regionCount, operationalObserved, operationalHealthy,
                    failureCount, unsafeFailureAction, signing, authenticated,
                    submitted, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<contract, fixtureCount, baselineDryRun, denialCount,
                    regionCount, operationalObserved, operationalHealthy,
                    failureCount, evaluated, certified, authorityGranted,
                    unsafeFailureAction, signing, authenticated, submitted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ RegisterContract \/ RecordFixture \/ RecordBaseline \/ RecordDenial
    \/ RecordRegion \/ \E healthy \in BOOLEAN: ObserveOperational(healthy)
    \/ SimulateFailure \/ Evaluate \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ contract \in BOOLEAN
    /\ fixtureCount \in 0..9
    /\ baselineDryRun \in BOOLEAN
    /\ denialCount \in 0..4
    /\ regionCount \in 0..2
    /\ operationalObserved \in BOOLEAN
    /\ operationalHealthy \in BOOLEAN
    /\ failureCount \in 0..8
    /\ evaluated \in BOOLEAN
    /\ certified \in BOOLEAN
    /\ authorityGranted \in BOOLEAN
    /\ unsafeFailureAction \in BOOLEAN
    /\ signing \in BOOLEAN
    /\ authenticated \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ halted \in BOOLEAN

CertificationComplete == certified =>
    contract /\ fixtureCount = 9 /\ baselineDryRun /\ denialCount = 4
    /\ regionCount = 2 /\ operationalObserved /\ operationalHealthy
    /\ failureCount = 8
SafeFailureActions == ~unsafeFailureAction
NoAuthority == ~authorityGranted /\ ~signing /\ ~authenticated /\ ~submitted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
