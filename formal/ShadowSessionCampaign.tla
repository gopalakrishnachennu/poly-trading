---------------- MODULE ShadowSessionCampaign ----------------
EXTENDS Naturals, TLC

VARIABLES registered, step, activeSession, completedSessions,
          certificationCovered, partitionCovered, deadManCovered,
          restartCovered, unknownPending, recoveryCovered, gatewayReady,
          unresolvedBacking, finalized, eligible, operatorDecisionRequired,
          promotionAuthority, deploymentAuthority, liveSubmission, halted

vars == <<registered, step, activeSession, completedSessions,
          certificationCovered, partitionCovered, deadManCovered,
          restartCovered, unknownPending, recoveryCovered, gatewayReady,
          unresolvedBacking, finalized, eligible, operatorDecisionRequired,
          promotionAuthority, deploymentAuthority, liveSubmission, halted>>

Init ==
    /\ registered = FALSE
    /\ step = 0
    /\ activeSession = 0
    /\ completedSessions = 0
    /\ certificationCovered = FALSE
    /\ partitionCovered = FALSE
    /\ deadManCovered = FALSE
    /\ restartCovered = FALSE
    /\ unknownPending = FALSE
    /\ recoveryCovered = FALSE
    /\ gatewayReady = FALSE
    /\ unresolvedBacking = FALSE
    /\ finalized = FALSE
    /\ eligible = FALSE
    /\ operatorDecisionRequired = FALSE
    /\ promotionAuthority = FALSE
    /\ deploymentAuthority = FALSE
    /\ liveSubmission = FALSE
    /\ halted = FALSE

Register ==
    /\ ~halted /\ ~registered
    /\ registered' = TRUE
    /\ step' = 1
    /\ UNCHANGED <<activeSession, completedSessions, certificationCovered,
                    partitionCovered, deadManCovered, restartCovered,
                    unknownPending, recoveryCovered, gatewayReady,
                    unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

OpenSession ==
    /\ ~halted /\ registered /\ ~finalized
    /\ step < 12
    /\ activeSession = 0 /\ completedSessions < 2
    /\ activeSession' = completedSessions + 1
    /\ step' = step + 1
    /\ UNCHANGED <<registered, completedSessions, certificationCovered,
                    partitionCovered, deadManCovered, restartCovered,
                    unknownPending, recoveryCovered, gatewayReady,
                    unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

CloseSession ==
    /\ ~halted /\ ~finalized
    /\ step < 12
    /\ activeSession = completedSessions + 1
    /\ activeSession' = 0
    /\ completedSessions' = completedSessions + 1
    /\ step' = step + 1
    /\ UNCHANGED <<registered, certificationCovered, partitionCovered,
                    deadManCovered, restartCovered, unknownPending,
                    recoveryCovered, gatewayReady, unresolvedBacking,
                    finalized, eligible, operatorDecisionRequired,
                    promotionAuthority, deploymentAuthority, liveSubmission,
                    halted>>

CoverCertification ==
    /\ ~halted /\ registered /\ ~finalized
    /\ step < 12
    /\ certificationCovered' = TRUE
    /\ gatewayReady' = TRUE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    partitionCovered, deadManCovered, restartCovered,
                    unknownPending, recoveryCovered, unresolvedBacking,
                    finalized, eligible, operatorDecisionRequired,
                    promotionAuthority, deploymentAuthority, liveSubmission,
                    halted>>

PartitionDeadMan ==
    /\ ~halted /\ registered /\ ~finalized
    /\ step < 12
    /\ partitionCovered' = TRUE
    /\ deadManCovered' = TRUE
    /\ gatewayReady' = FALSE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    certificationCovered, restartCovered, unknownPending,
                    recoveryCovered, unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

RestartUnknown ==
    /\ ~halted /\ registered /\ ~finalized
    /\ step < 12
    /\ restartCovered' = TRUE
    /\ unknownPending' = TRUE
    /\ gatewayReady' = FALSE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    recoveryCovered, unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

RecoverUnknown ==
    /\ ~halted /\ ~finalized /\ unknownPending
    /\ step < 12
    /\ certificationCovered
    /\ unknownPending' = FALSE
    /\ recoveryCovered' = TRUE
    /\ gatewayReady' = TRUE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    restartCovered, unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

CreateBacking ==
    /\ ~halted /\ registered /\ ~finalized /\ activeSession > 0
    /\ step < 12
    /\ gatewayReady /\ ~unresolvedBacking
    /\ unresolvedBacking' = TRUE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    restartCovered, unknownPending, recoveryCovered,
                    gatewayReady, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

ResolveBacking ==
    /\ ~halted /\ ~finalized /\ unresolvedBacking
    /\ step < 12
    /\ unresolvedBacking' = FALSE
    /\ step' = step + 1
    /\ UNCHANGED <<registered, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    restartCovered, unknownPending, recoveryCovered,
                    gatewayReady, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ eligible' = (completedSessions = 2 /\ activeSession = 0
                     /\ certificationCovered /\ partitionCovered
                     /\ deadManCovered /\ restartCovered /\ recoveryCovered
                     /\ ~unknownPending /\ gatewayReady /\ ~unresolvedBacking)
    /\ operatorDecisionRequired' = TRUE
    /\ promotionAuthority' = FALSE
    /\ deploymentAuthority' = FALSE
    /\ UNCHANGED <<registered, step, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    restartCovered, unknownPending, recoveryCovered,
                    gatewayReady, unresolvedBacking, liveSubmission, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<registered, step, activeSession, completedSessions,
                    certificationCovered, partitionCovered, deadManCovered,
                    restartCovered, unknownPending, recoveryCovered,
                    gatewayReady, unresolvedBacking, finalized, eligible,
                    operatorDecisionRequired, promotionAuthority,
                    deploymentAuthority, liveSubmission>>

Halted == halted /\ UNCHANGED vars

Next == Register \/ OpenSession \/ CloseSession \/ CoverCertification
        \/ PartitionDeadMan \/ RestartUnknown \/ RecoverUnknown
        \/ CreateBacking \/ ResolveBacking \/ Finalize
        \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ registered \in BOOLEAN
    /\ step \in 0..12
    /\ activeSession \in 0..2
    /\ completedSessions \in 0..2
    /\ certificationCovered \in BOOLEAN
    /\ partitionCovered \in BOOLEAN
    /\ deadManCovered \in BOOLEAN
    /\ restartCovered \in BOOLEAN
    /\ unknownPending \in BOOLEAN
    /\ recoveryCovered \in BOOLEAN
    /\ gatewayReady \in BOOLEAN
    /\ unresolvedBacking \in BOOLEAN
    /\ finalized \in BOOLEAN
    /\ eligible \in BOOLEAN
    /\ operatorDecisionRequired \in BOOLEAN
    /\ promotionAuthority \in BOOLEAN
    /\ deploymentAuthority \in BOOLEAN
    /\ liveSubmission \in BOOLEAN
    /\ halted \in BOOLEAN

SessionOrder == activeSession > 0 => activeSession = completedSessions + 1
EligibilityComplete == eligible =>
    finalized /\ completedSessions = 2 /\ activeSession = 0
    /\ certificationCovered /\ partitionCovered /\ deadManCovered
    /\ restartCovered /\ recoveryCovered /\ ~unknownPending
    /\ gatewayReady /\ ~unresolvedBacking
EvidenceNeedsOperator == eligible => operatorDecisionRequired
NoAutomaticAuthority == ~promotionAuthority /\ ~deploymentAuthority /\ ~liveSubmission
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
