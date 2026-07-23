---------------- MODULE CanaryRolloutSimulator ----------------
EXTENDS Naturals, TLC

VARIABLES registered, mode, stage, healthReady, inWindow, rollbackLatched,
          restartSeen, recoverySeen, finalized, rolloutAuthority,
          rollbackAuthority, deploymentAuthority, credentialAuthority,
          liveTradingAuthority, halted

vars == <<registered, mode, stage, healthReady, inWindow, rollbackLatched,
          restartSeen, recoverySeen, finalized, rolloutAuthority,
          rollbackAuthority, deploymentAuthority, credentialAuthority,
          liveTradingAuthority, halted>>

TerminalModes == {"completed", "aborted", "rollback"}

Init ==
    /\ registered = FALSE
    /\ mode = "none"
    /\ stage = 0
    /\ healthReady = FALSE
    /\ inWindow = FALSE
    /\ rollbackLatched = FALSE
    /\ restartSeen = FALSE
    /\ recoverySeen = FALSE
    /\ finalized = FALSE
    /\ rolloutAuthority = FALSE
    /\ rollbackAuthority = FALSE
    /\ deploymentAuthority = FALSE
    /\ credentialAuthority = FALSE
    /\ liveTradingAuthority = FALSE
    /\ halted = FALSE

Register ==
    /\ ~halted /\ ~registered
    /\ registered' = TRUE
    /\ mode' = "registered"
    /\ UNCHANGED <<stage, healthReady, inWindow, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Healthy ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ healthReady' = TRUE
    /\ UNCHANGED <<registered, mode, stage, inWindow, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Unhealthy ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ healthReady' = FALSE
    /\ mode' = IF mode = "running" THEN "paused" ELSE mode
    /\ UNCHANGED <<registered, stage, inWindow, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

EnterWindow ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ inWindow' = TRUE
    /\ UNCHANGED <<registered, mode, stage, healthReady, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

LeaveWindow ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ inWindow' = FALSE
    /\ mode' = IF mode = "running" THEN "paused" ELSE mode
    /\ UNCHANGED <<registered, stage, healthReady, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Start ==
    /\ ~halted /\ mode = "registered" /\ healthReady /\ inWindow
    /\ mode' = "running"
    /\ stage' = 1
    /\ UNCHANGED <<registered, healthReady, inWindow, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Advance ==
    /\ ~halted /\ mode = "running" /\ healthReady /\ inWindow
    /\ stage \in 1..2
    /\ stage' = IF stage = 1 THEN 2 ELSE stage
    /\ mode' = IF stage = 2 THEN "completed" ELSE mode
    /\ UNCHANGED <<registered, healthReady, inWindow, rollbackLatched,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Pause ==
    /\ ~halted /\ mode = "running"
    /\ mode' = "paused"
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, recoverySeen, finalized,
                    rolloutAuthority, rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Resume ==
    /\ ~halted /\ mode = "paused" /\ healthReady /\ inWindow
    /\ mode' = "running"
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, recoverySeen, finalized,
                    rolloutAuthority, rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

SevereFailure ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ mode' = "rollback"
    /\ rollbackLatched' = TRUE
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    restartSeen, recoverySeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Restart ==
    /\ ~halted /\ mode \in {"running", "paused"}
    /\ mode' = "recovering"
    /\ restartSeen' = TRUE
    /\ recoverySeen' = FALSE
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    rollbackLatched, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Recover ==
    /\ ~halted /\ mode = "recovering" /\ restartSeen /\ healthReady
    /\ mode' = "paused"
    /\ recoverySeen' = TRUE
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, finalized, rolloutAuthority,
                    rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Abort ==
    /\ ~halted /\ registered /\ mode \notin TerminalModes
    /\ mode' = "aborted"
    /\ UNCHANGED <<registered, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, recoverySeen, finalized,
                    rolloutAuthority, rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ ~finalized /\ mode \in TerminalModes
    /\ finalized' = TRUE
    /\ UNCHANGED <<registered, mode, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, recoverySeen,
                    rolloutAuthority, rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<registered, mode, stage, healthReady, inWindow,
                    rollbackLatched, restartSeen, recoverySeen, finalized,
                    rolloutAuthority, rollbackAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars

Next == Register \/ Healthy \/ Unhealthy \/ EnterWindow \/ LeaveWindow
        \/ Start \/ Advance \/ Pause \/ Resume \/ SevereFailure
        \/ Restart \/ Recover \/ Abort \/ Finalize \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ registered \in BOOLEAN
    /\ mode \in {"none", "registered", "running", "paused", "recovering",
                   "completed", "aborted", "rollback"}
    /\ stage \in 0..2
    /\ healthReady \in BOOLEAN
    /\ inWindow \in BOOLEAN
    /\ rollbackLatched \in BOOLEAN
    /\ restartSeen \in BOOLEAN
    /\ recoverySeen \in BOOLEAN
    /\ finalized \in BOOLEAN
    /\ rolloutAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN
    /\ deploymentAuthority \in BOOLEAN
    /\ credentialAuthority \in BOOLEAN
    /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RunningRequiresGates == mode = "running" => (healthReady /\ inWindow /\ stage > 0)

RollbackIsLatched == mode = "rollback" => rollbackLatched

RecoveringRequiresRestart == mode = "recovering" => restartSeen

RecoveryFollowsRestart == recoverySeen => restartSeen

FinalizedOnlyTerminal == finalized => mode \in TerminalModes

NoAuthority ==
    /\ ~rolloutAuthority
    /\ ~rollbackAuthority
    /\ ~deploymentAuthority
    /\ ~credentialAuthority
    /\ ~liveTradingAuthority

=============================================================================
