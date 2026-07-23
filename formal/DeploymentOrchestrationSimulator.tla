---------------- MODULE DeploymentOrchestrationSimulator ----------------
EXTENDS Naturals, TLC

VARIABLES planValid, registered, healthCurrent, mode, wave, activated,
          rolledBack, priorRollback, recoveryEpoch, finalized,
          credentialCreated, deploymentAuthority, rollbackAuthority,
          cloudAuthority, liveTradingAuthority, halted

vars == <<planValid, registered, healthCurrent, mode, wave, activated,
          rolledBack, priorRollback, recoveryEpoch, finalized,
          credentialCreated, deploymentAuthority, rollbackAuthority,
          cloudAuthority, liveTradingAuthority, halted>>

Init ==
    /\ planValid = FALSE /\ registered = FALSE /\ healthCurrent = FALSE
    /\ mode = "Empty" /\ wave = 0 /\ activated = 0 /\ rolledBack = 0
    /\ priorRollback = FALSE /\ recoveryEpoch = 0 /\ finalized = FALSE
    /\ credentialCreated = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ cloudAuthority = FALSE
    /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidatePlan ==
    /\ ~halted /\ ~registered /\ planValid' = TRUE
    /\ UNCHANGED <<registered, healthCurrent, mode, wave, activated, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Register ==
    /\ ~halted /\ planValid /\ ~registered
    /\ registered' = TRUE /\ mode' = "Registered"
    /\ UNCHANGED <<planValid, healthCurrent, wave, activated, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

ObserveHealthy ==
    /\ ~halted /\ registered /\ ~finalized /\ healthCurrent' = TRUE
    /\ UNCHANGED <<planValid, registered, mode, wave, activated, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Start ==
    /\ ~halted /\ mode = "Registered" /\ healthCurrent
    /\ mode' = "Running" /\ wave' = 1 /\ activated' = 1
    /\ UNCHANGED <<planValid, registered, healthCurrent, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Advance ==
    /\ ~halted /\ mode = "Running" /\ healthCurrent /\ wave < 2
    /\ wave' = wave + 1 /\ activated' = activated + 1
    /\ UNCHANGED <<planValid, registered, healthCurrent, mode, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Complete ==
    /\ ~halted /\ mode = "Running" /\ healthCurrent /\ wave = 2
    /\ mode' = "Completed"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Degrade ==
    /\ ~halted /\ mode = "Running"
    /\ healthCurrent' = FALSE /\ mode' = "Paused"
    /\ UNCHANGED <<planValid, registered, wave, activated, rolledBack,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Resume ==
    /\ ~halted /\ mode = "Paused" /\ healthCurrent /\ mode' = "Running"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RequireRollback ==
    /\ ~halted /\ activated > 0
    /\ mode \in {"Running", "Paused"} /\ mode' = "RollbackRequired"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

AbortBeforeStart ==
    /\ ~halted /\ mode = "Registered" /\ mode' = "Aborted"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Restart ==
    /\ ~halted /\ mode \in {"Running", "Paused", "RollbackRequired"}
    /\ priorRollback' = (mode = "RollbackRequired") /\ mode' = "Recovering"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Recover ==
    /\ ~halted /\ mode = "Recovering" /\ recoveryEpoch < 2
    /\ recoveryEpoch' = recoveryEpoch + 1
    /\ mode' = IF priorRollback THEN "RollbackRequired" ELSE "Paused"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    rolledBack, priorRollback, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

RollbackStep ==
    /\ ~halted /\ mode = "RollbackRequired" /\ rolledBack < activated
    /\ rolledBack' = rolledBack + 1
    /\ mode' = IF rolledBack + 1 = activated THEN "RolledBack" ELSE "RollbackRequired"
    /\ UNCHANGED <<planValid, registered, healthCurrent, wave, activated,
                    priorRollback, recoveryEpoch, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ mode \in {"Completed", "RolledBack", "Aborted"}
    /\ finalized' = TRUE
    /\ UNCHANGED <<planValid, registered, healthCurrent, mode, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<planValid, registered, healthCurrent, mode, wave, activated,
                    rolledBack, priorRollback, recoveryEpoch, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars

Next == ValidatePlan \/ Register \/ ObserveHealthy \/ Start \/ Advance \/ Complete
        \/ Degrade \/ Resume \/ RequireRollback \/ AbortBeforeStart \/ Restart
        \/ Recover \/ RollbackStep \/ Finalize \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ planValid \in BOOLEAN /\ registered \in BOOLEAN /\ healthCurrent \in BOOLEAN
    /\ mode \in {"Empty", "Registered", "Running", "Paused", "Recovering",
                  "RollbackRequired", "RolledBack", "Completed", "Aborted"}
    /\ wave \in 0..2 /\ activated \in 0..2 /\ rolledBack \in 0..2
    /\ priorRollback \in BOOLEAN /\ recoveryEpoch \in 0..2 /\ finalized \in BOOLEAN
    /\ credentialCreated \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN /\ cloudAuthority \in BOOLEAN
    /\ liveTradingAuthority \in BOOLEAN /\ halted \in BOOLEAN

RegisteredRequiresValidPlan == registered => planValid
WaveOrder == wave = activated
RollbackBound == rolledBack <= activated
RunningRequiresHealth == mode = "Running" => healthCurrent
RollbackConvergence == mode = "RolledBack" => rolledBack = activated
FinalizedTerminal == finalized => mode \in {"Completed", "RolledBack", "Aborted"}
NoAuthority ==
    /\ ~credentialCreated /\ ~deploymentAuthority /\ ~rollbackAuthority
    /\ ~cloudAuthority /\ ~liveTradingAuthority

=============================================================================
