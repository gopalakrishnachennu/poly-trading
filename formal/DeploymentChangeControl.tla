-------------------- MODULE DeploymentChangeControl --------------------
EXTENDS Naturals, TLC

CONSTANT StepCount

VARIABLES certificateValid, registered, releaseApproved, riskApproved,
          operatorsDistinct, mode, permissionIssued, permissionInvalidated,
          consumedSteps, rolledBackSteps, finalized, credentialCreated,
          deploymentAuthority, rollbackAuthority, cloudAuthority,
          liveTradingAuthority, halted

vars == <<certificateValid, registered, releaseApproved, riskApproved,
          operatorsDistinct, mode, permissionIssued, permissionInvalidated,
          consumedSteps, rolledBackSteps, finalized, credentialCreated,
          deploymentAuthority, rollbackAuthority, cloudAuthority,
          liveTradingAuthority, halted>>

TerminalModes == {"COMPLETED", "ABORTED", "ROLLED_BACK"}

Init ==
    /\ certificateValid = FALSE /\ registered = FALSE
    /\ releaseApproved = FALSE /\ riskApproved = FALSE
    /\ operatorsDistinct = FALSE /\ mode = "EMPTY"
    /\ permissionIssued = FALSE /\ permissionInvalidated = FALSE
    /\ consumedSteps = 0 /\ rolledBackSteps = 0 /\ finalized = FALSE
    /\ credentialCreated = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ cloudAuthority = FALSE
    /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidateCertificate ==
    /\ ~halted /\ ~registered /\ certificateValid' = TRUE
    /\ UNCHANGED <<registered, releaseApproved, riskApproved,
                    operatorsDistinct, mode, permissionIssued,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    finalized, credentialCreated, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority,
                    halted>>

Register ==
    /\ ~halted /\ ~registered /\ certificateValid
    /\ registered' = TRUE /\ mode' = "REGISTERED"
    /\ UNCHANGED <<certificateValid, releaseApproved, riskApproved,
                    operatorsDistinct, permissionIssued, permissionInvalidated,
                    consumedSteps, rolledBackSteps, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveRelease ==
    /\ ~halted /\ registered /\ ~finalized
    /\ releaseApproved' = TRUE
    /\ UNCHANGED <<certificateValid, registered, riskApproved,
                    operatorsDistinct, mode, permissionIssued,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    finalized, credentialCreated, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority,
                    halted>>

ApproveRiskDistinct ==
    /\ ~halted /\ registered /\ ~finalized /\ releaseApproved
    /\ riskApproved' = TRUE /\ operatorsDistinct' = TRUE
    /\ mode' = "APPROVED"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    permissionIssued, permissionInvalidated, consumedSteps,
                    rolledBackSteps, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

IssueChangePermission ==
    /\ ~halted /\ registered /\ ~finalized
    /\ mode \in {"APPROVED", "ACTIVE"}
    /\ releaseApproved /\ riskApproved /\ operatorsDistinct
    /\ ~permissionIssued /\ consumedSteps < StepCount
    /\ permissionIssued' = TRUE /\ mode' = "ACTIVE"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, permissionInvalidated,
                    consumedSteps, rolledBackSteps, finalized,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ConsumeChangePermission ==
    /\ ~halted /\ permissionIssued /\ mode = "ACTIVE"
    /\ permissionIssued' = FALSE
    /\ consumedSteps' = consumedSteps + 1
    /\ mode' = IF consumedSteps + 1 = StepCount
                THEN "COMPLETED" ELSE "ACTIVE"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, permissionInvalidated,
                    rolledBackSteps, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Pause ==
    /\ ~halted /\ registered /\ ~finalized /\ mode \notin TerminalModes
    /\ mode' = "PAUSED"
    /\ permissionInvalidated' = (permissionInvalidated \/ permissionIssued)
    /\ permissionIssued' = FALSE
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, consumedSteps,
                    rolledBackSteps, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Resume ==
    /\ ~halted /\ mode = "PAUSED"
    /\ releaseApproved /\ riskApproved /\ operatorsDistinct
    /\ mode' = IF consumedSteps = 0 THEN "APPROVED" ELSE "ACTIVE"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, permissionIssued,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    finalized, credentialCreated, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority,
                    halted>>

AbortOrEmergency ==
    /\ ~halted /\ registered /\ ~finalized /\ mode \notin TerminalModes
    /\ permissionIssued' = FALSE
    /\ permissionInvalidated' = (permissionInvalidated \/ permissionIssued)
    /\ mode' = IF consumedSteps = 0 THEN "ABORTED" ELSE "ROLLBACK_REQUIRED"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, consumedSteps,
                    rolledBackSteps, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

IssueRollbackPermission ==
    /\ ~halted /\ mode = "ROLLBACK_REQUIRED" /\ ~permissionIssued
    /\ rolledBackSteps < consumedSteps
    /\ permissionIssued' = TRUE
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, mode,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    finalized, credentialCreated, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority,
                    halted>>

ConsumeRollbackPermission ==
    /\ ~halted /\ mode = "ROLLBACK_REQUIRED" /\ permissionIssued
    /\ permissionIssued' = FALSE /\ rolledBackSteps' = rolledBackSteps + 1
    /\ mode' = IF rolledBackSteps + 1 = consumedSteps
                THEN "ROLLED_BACK" ELSE "ROLLBACK_REQUIRED"
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, permissionInvalidated,
                    consumedSteps, finalized, credentialCreated,
                    deploymentAuthority, rollbackAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ mode \in TerminalModes /\ ~finalized
    /\ finalized' = TRUE
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, mode, permissionIssued,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    credentialCreated, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE /\ permissionIssued' = FALSE
    /\ UNCHANGED <<certificateValid, registered, releaseApproved,
                    riskApproved, operatorsDistinct, mode,
                    permissionInvalidated, consumedSteps, rolledBackSteps,
                    finalized, credentialCreated, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars
Terminal == finalized /\ UNCHANGED vars

Next == ValidateCertificate \/ Register \/ ApproveRelease
        \/ ApproveRiskDistinct \/ IssueChangePermission
        \/ ConsumeChangePermission \/ Pause \/ Resume \/ AbortOrEmergency
        \/ IssueRollbackPermission \/ ConsumeRollbackPermission \/ Finalize
        \/ IntegrityFailure \/ Halted \/ Terminal

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ certificateValid \in BOOLEAN /\ registered \in BOOLEAN
    /\ releaseApproved \in BOOLEAN /\ riskApproved \in BOOLEAN
    /\ operatorsDistinct \in BOOLEAN
    /\ mode \in {"EMPTY", "REGISTERED", "APPROVED", "ACTIVE", "PAUSED",
                  "ROLLBACK_REQUIRED", "COMPLETED", "ABORTED", "ROLLED_BACK"}
    /\ permissionIssued \in BOOLEAN /\ permissionInvalidated \in BOOLEAN
    /\ consumedSteps \in 0..StepCount /\ rolledBackSteps \in 0..StepCount
    /\ finalized \in BOOLEAN /\ credentialCreated \in BOOLEAN
    /\ deploymentAuthority \in BOOLEAN /\ rollbackAuthority \in BOOLEAN
    /\ cloudAuthority \in BOOLEAN /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegisteredRequiresCertificate == registered => certificateValid
PermissionRequiresDualControl ==
    permissionIssued => (releaseApproved /\ riskApproved /\ operatorsDistinct)
RollbackNeverExceedsApplied == rolledBackSteps <= consumedSteps
CompletedRequiresAllSteps == mode = "COMPLETED" => consumedSteps = StepCount
RolledBackRequiresConvergence ==
    mode = "ROLLED_BACK" => rolledBackSteps = consumedSteps
FinalizedRequiresTerminal == finalized => mode \in TerminalModes
NoExternalAuthority ==
    /\ ~credentialCreated /\ ~deploymentAuthority /\ ~rollbackAuthority
    /\ ~cloudAuthority /\ ~liveTradingAuthority

=============================================================================
