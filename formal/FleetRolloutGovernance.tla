---------------- MODULE FleetRolloutGovernance ----------------
EXTENDS Naturals, TLC

VARIABLES registered, regionsComplete, abortCovered, rollbackCovered,
          triggerCovered, freezeActive, revoked, revocationLatched,
          finalized, ready, fleetAuthority, deploymentAuthority,
          rollbackAuthority, credentialAuthority, liveTradingAuthority, halted

vars == <<registered, regionsComplete, abortCovered, rollbackCovered,
          triggerCovered, freezeActive, revoked, revocationLatched,
          finalized, ready, fleetAuthority, deploymentAuthority,
          rollbackAuthority, credentialAuthority, liveTradingAuthority, halted>>

Init ==
    /\ registered = FALSE /\ regionsComplete = FALSE /\ abortCovered = FALSE
    /\ rollbackCovered = FALSE /\ triggerCovered = FALSE /\ freezeActive = FALSE
    /\ revoked = FALSE /\ revocationLatched = FALSE /\ finalized = FALSE
    /\ ready = FALSE /\ fleetAuthority = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ credentialAuthority = FALSE
    /\ liveTradingAuthority = FALSE /\ halted = FALSE

Register ==
    /\ ~halted /\ ~registered
    /\ registered' = TRUE
    /\ UNCHANGED <<regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, freezeActive, revoked, revocationLatched,
                    finalized, ready, fleetAuthority, deploymentAuthority,
                    rollbackAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

CompleteRegions ==
    /\ ~halted /\ registered /\ ~finalized
    /\ regionsComplete' = TRUE
    /\ UNCHANGED <<registered, abortCovered, rollbackCovered, triggerCovered,
                    freezeActive, revoked, revocationLatched, finalized, ready,
                    fleetAuthority, deploymentAuthority, rollbackAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

CoverAbort ==
    /\ ~halted /\ registered /\ ~finalized
    /\ abortCovered' = TRUE
    /\ UNCHANGED <<registered, regionsComplete, rollbackCovered,
                    triggerCovered, freezeActive, revoked, revocationLatched,
                    finalized, ready, fleetAuthority, deploymentAuthority,
                    rollbackAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

CoverRollback ==
    /\ ~halted /\ registered /\ ~finalized
    /\ rollbackCovered' = TRUE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, triggerCovered,
                    freezeActive, revoked, revocationLatched, finalized, ready,
                    fleetAuthority, deploymentAuthority, rollbackAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

CoverTriggers ==
    /\ ~halted /\ registered /\ ~finalized
    /\ triggerCovered' = TRUE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    freezeActive, revoked, revocationLatched, finalized, ready,
                    fleetAuthority, deploymentAuthority, rollbackAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

EnterFreeze ==
    /\ ~halted /\ registered /\ ~finalized
    /\ freezeActive' = TRUE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, revoked, revocationLatched, finalized,
                    ready, fleetAuthority, deploymentAuthority,
                    rollbackAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

LeaveFreeze ==
    /\ ~halted /\ registered /\ ~finalized
    /\ freezeActive' = FALSE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, revoked, revocationLatched, finalized,
                    ready, fleetAuthority, deploymentAuthority,
                    rollbackAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

Revoke ==
    /\ ~halted /\ registered /\ ~revoked
    /\ revoked' = TRUE /\ revocationLatched' = TRUE
    /\ finalized' = FALSE /\ ready' = FALSE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, freezeActive, fleetAuthority,
                    deploymentAuthority, rollbackAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ ready' = regionsComplete /\ abortCovered /\ rollbackCovered
                 /\ triggerCovered /\ freezeActive /\ ~revoked
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, freezeActive, revoked, revocationLatched,
                    fleetAuthority, deploymentAuthority, rollbackAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<registered, regionsComplete, abortCovered, rollbackCovered,
                    triggerCovered, freezeActive, revoked, revocationLatched,
                    finalized, ready, fleetAuthority, deploymentAuthority,
                    rollbackAuthority, credentialAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars

Next == Register \/ CompleteRegions \/ CoverAbort \/ CoverRollback
        \/ CoverTriggers \/ EnterFreeze \/ LeaveFreeze \/ Revoke
        \/ Finalize \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ registered \in BOOLEAN /\ regionsComplete \in BOOLEAN
    /\ abortCovered \in BOOLEAN /\ rollbackCovered \in BOOLEAN
    /\ triggerCovered \in BOOLEAN /\ freezeActive \in BOOLEAN
    /\ revoked \in BOOLEAN /\ revocationLatched \in BOOLEAN
    /\ finalized \in BOOLEAN /\ ready \in BOOLEAN
    /\ fleetAuthority \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN /\ credentialAuthority \in BOOLEAN
    /\ liveTradingAuthority \in BOOLEAN /\ halted \in BOOLEAN

ReadyRequiresAllGates ==
    ready => (finalized /\ regionsComplete /\ abortCovered /\ rollbackCovered
              /\ triggerCovered /\ freezeActive /\ ~revoked)

RevocationIsLatched == revoked => revocationLatched
RevocationDeniesReadiness == revoked => ~ready

NoAuthority ==
    /\ ~fleetAuthority /\ ~deploymentAuthority /\ ~rollbackAuthority
    /\ ~credentialAuthority /\ ~liveTradingAuthority

=============================================================================
