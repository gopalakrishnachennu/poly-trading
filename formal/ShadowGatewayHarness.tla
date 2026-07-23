---------------- MODULE ShadowGatewayHarness ----------------
EXTENDS Naturals, TLC

Modes == {"Starting", "Disabled", "Ready", "PostOnly", "Restarting",
          "Recovering", "DeadMan", "Halted"}

VARIABLES certified, certificationFresh, heartbeatPresent, heartbeatHealthy,
          heartbeatFresh, mode, restartPending, reconciliationCurrent,
          unknownOrdersCleared, exposureCount, reservations, unsafeExposure,
          automaticRetry, backingReleased, signing, authenticated,
          liveSubmission, halted

vars == <<certified, certificationFresh, heartbeatPresent, heartbeatHealthy,
          heartbeatFresh, mode, restartPending, reconciliationCurrent,
          unknownOrdersCleared, exposureCount, reservations, unsafeExposure,
          automaticRetry, backingReleased, signing, authenticated,
          liveSubmission, halted>>

Init ==
    /\ certified = FALSE
    /\ certificationFresh = FALSE
    /\ heartbeatPresent = FALSE
    /\ heartbeatHealthy = FALSE
    /\ heartbeatFresh = FALSE
    /\ mode = "Starting"
    /\ restartPending = FALSE
    /\ reconciliationCurrent = FALSE
    /\ unknownOrdersCleared = FALSE
    /\ exposureCount = 0
    /\ reservations = 0
    /\ unsafeExposure = FALSE
    /\ automaticRetry = FALSE
    /\ backingReleased = FALSE
    /\ signing = FALSE
    /\ authenticated = FALSE
    /\ liveSubmission = FALSE
    /\ halted = FALSE

InstallCertification(ok, fresh) ==
    /\ ~halted
    /\ certified' = ok
    /\ certificationFresh' = ok /\ fresh
    /\ mode' = IF ok /\ fresh /\ heartbeatPresent /\ heartbeatHealthy
                   /\ heartbeatFresh /\ ~restartPending
                THEN "Ready" ELSE "Disabled"
    /\ UNCHANGED <<heartbeatPresent, heartbeatHealthy, heartbeatFresh,
                    restartPending, reconciliationCurrent, unknownOrdersCleared,
                    exposureCount, reservations, unsafeExposure, automaticRetry,
                    backingReleased, signing, authenticated, liveSubmission,
                    halted>>

ObserveHeartbeat(healthy, fresh) ==
    /\ ~halted
    /\ heartbeatPresent' = TRUE
    /\ heartbeatHealthy' = healthy
    /\ heartbeatFresh' = fresh
    /\ mode' = IF healthy /\ fresh /\ certified /\ certificationFresh
                   /\ ~restartPending
                THEN "Ready" ELSE "DeadMan"
    /\ UNCHANGED <<certified, certificationFresh, restartPending,
                    reconciliationCurrent, unknownOrdersCleared, exposureCount,
                    reservations, unsafeExposure, automaticRetry,
                    backingReleased, signing, authenticated, liveSubmission,
                    halted>>

ExpireCertification ==
    /\ ~halted
    /\ certified
    /\ certificationFresh
    /\ certificationFresh' = FALSE
    /\ mode' = "Disabled"
    /\ UNCHANGED <<certified, heartbeatPresent, heartbeatHealthy,
                    heartbeatFresh, restartPending, reconciliationCurrent,
                    unknownOrdersCleared, exposureCount, reservations,
                    unsafeExposure, automaticRetry, backingReleased, signing,
                    authenticated, liveSubmission, halted>>

LoseHeartbeat ==
    /\ ~halted
    /\ heartbeatPresent
    /\ heartbeatFresh' = FALSE
    /\ mode' = "DeadMan"
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, restartPending, reconciliationCurrent,
                    unknownOrdersCleared, exposureCount, reservations,
                    unsafeExposure, automaticRetry, backingReleased, signing,
                    authenticated, liveSubmission, halted>>

RestartFixture ==
    /\ ~halted
    /\ restartPending' = TRUE
    /\ reconciliationCurrent' = FALSE
    /\ unknownOrdersCleared' = FALSE
    /\ mode' = "Restarting"
    /\ automaticRetry' = FALSE
    /\ backingReleased' = FALSE
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, exposureCount,
                    reservations, unsafeExposure, signing, authenticated,
                    liveSubmission, halted>>

AdverseFixture ==
    /\ ~halted
    /\ mode' = "Recovering"
    /\ automaticRetry' = FALSE
    /\ backingReleased' = FALSE
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, restartPending,
                    reconciliationCurrent, unknownOrdersCleared, exposureCount,
                    reservations, unsafeExposure, signing, authenticated,
                    liveSubmission, halted>>

ObserveRecoveryEvidence(reconciled, cleared) ==
    /\ ~halted
    /\ reconciliationCurrent' = reconciled
    /\ unknownOrdersCleared' = cleared
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, mode, restartPending,
                    exposureCount, reservations, unsafeExposure, automaticRetry,
                    backingReleased, signing, authenticated, liveSubmission,
                    halted>>

Recover ==
    /\ ~halted
    /\ restartPending
    /\ certified /\ certificationFresh
    /\ heartbeatPresent /\ heartbeatHealthy /\ heartbeatFresh
    /\ reconciliationCurrent /\ unknownOrdersCleared
    /\ restartPending' = FALSE
    /\ mode' = "Ready"
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, reconciliationCurrent,
                    unknownOrdersCleared, exposureCount, reservations,
                    unsafeExposure, automaticRetry, backingReleased, signing,
                    authenticated, liveSubmission, halted>>

CreateShadowExposure ==
    /\ ~halted
    /\ certified /\ certificationFresh
    /\ heartbeatPresent /\ heartbeatHealthy /\ heartbeatFresh
    /\ mode \in {"Ready", "PostOnly"}
    /\ ~restartPending
    /\ exposureCount < 2
    /\ exposureCount' = exposureCount + 1
    /\ reservations' = reservations + 1
    /\ unsafeExposure' = FALSE
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, mode, restartPending,
                    reconciliationCurrent, unknownOrdersCleared, automaticRetry,
                    backingReleased, signing, authenticated, liveSubmission,
                    halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ mode' = "Halted"
    /\ UNCHANGED <<certified, certificationFresh, heartbeatPresent,
                    heartbeatHealthy, heartbeatFresh, restartPending,
                    reconciliationCurrent, unknownOrdersCleared, exposureCount,
                    reservations, unsafeExposure, automaticRetry,
                    backingReleased, signing, authenticated, liveSubmission>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ \E ok \in BOOLEAN, fresh \in BOOLEAN: InstallCertification(ok, fresh)
    \/ \E healthy \in BOOLEAN, fresh \in BOOLEAN: ObserveHeartbeat(healthy, fresh)
    \/ ExpireCertification \/ LoseHeartbeat \/ RestartFixture
    \/ AdverseFixture
    \/ \E reconciled \in BOOLEAN, cleared \in BOOLEAN:
         ObserveRecoveryEvidence(reconciled, cleared)
    \/ Recover \/ CreateShadowExposure \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ certified \in BOOLEAN
    /\ certificationFresh \in BOOLEAN
    /\ heartbeatPresent \in BOOLEAN
    /\ heartbeatHealthy \in BOOLEAN
    /\ heartbeatFresh \in BOOLEAN
    /\ mode \in Modes
    /\ restartPending \in BOOLEAN
    /\ reconciliationCurrent \in BOOLEAN
    /\ unknownOrdersCleared \in BOOLEAN
    /\ exposureCount \in 0..2
    /\ reservations \in 0..2
    /\ unsafeExposure \in BOOLEAN
    /\ automaticRetry \in BOOLEAN
    /\ backingReleased \in BOOLEAN
    /\ signing \in BOOLEAN
    /\ authenticated \in BOOLEAN
    /\ liveSubmission \in BOOLEAN
    /\ halted \in BOOLEAN

ExposureIsBacked == reservations = exposureCount
NoUnsafeExposure == ~unsafeExposure
ExpiryDisables == ~certificationFresh => mode # "Ready"
HeartbeatFailureDisables ==
    (~heartbeatPresent \/ ~heartbeatHealthy \/ ~heartbeatFresh) => mode # "Ready"
RestartRequiresRecovery == restartPending => mode # "Ready"
FixturesAreConservative == ~automaticRetry /\ ~backingReleased
NoLiveAuthority == ~signing /\ ~authenticated /\ ~liveSubmission
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
