-------------------- MODULE DeploymentChangeCampaign --------------------
EXTENDS Naturals, TLC

CONSTANT CaseCount

VARIABLES manifestValid, registered, completedCases, schedulePosition,
          multiWindow, approvalRenewal, approvalExpiry, pauseResume,
          safeAbort, emergencyRollback, restartRecovered, approvalSets,
          finalized, eligible, operatorDecisionRequired, credentialCreated,
          authenticationAuthority, deploymentAuthority, rollbackAuthority,
          trafficAuthority, cloudAuthority, liveTradingAuthority, halted

vars == <<manifestValid, registered, completedCases, schedulePosition,
          multiWindow, approvalRenewal, approvalExpiry, pauseResume,
          safeAbort, emergencyRollback, restartRecovered, approvalSets,
          finalized, eligible, operatorDecisionRequired, credentialCreated,
          authenticationAuthority, deploymentAuthority, rollbackAuthority,
          trafficAuthority, cloudAuthority, liveTradingAuthority, halted>>

AllCoverage == multiWindow /\ approvalRenewal /\ approvalExpiry /\ pauseResume
               /\ safeAbort /\ emergencyRollback /\ restartRecovered

Init ==
    /\ manifestValid = FALSE /\ registered = FALSE
    /\ completedCases = 0 /\ schedulePosition = 0
    /\ multiWindow = FALSE /\ approvalRenewal = FALSE
    /\ approvalExpiry = FALSE /\ pauseResume = FALSE
    /\ safeAbort = FALSE /\ emergencyRollback = FALSE
    /\ restartRecovered = FALSE /\ approvalSets = 0
    /\ finalized = FALSE /\ eligible = FALSE
    /\ operatorDecisionRequired = TRUE /\ credentialCreated = FALSE
    /\ authenticationAuthority = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ trafficAuthority = FALSE
    /\ cloudAuthority = FALSE /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidateManifest ==
    /\ ~halted /\ ~registered /\ manifestValid' = TRUE
    /\ UNCHANGED <<registered, completedCases, schedulePosition, multiWindow,
                    approvalRenewal, approvalExpiry, pauseResume, safeAbort,
                    emergencyRollback, restartRecovered, approvalSets,
                    finalized, eligible, operatorDecisionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Register ==
    /\ ~halted /\ ~registered /\ manifestValid
    /\ registered' = TRUE
    /\ UNCHANGED <<manifestValid, completedCases, schedulePosition, multiWindow,
                    approvalRenewal, approvalExpiry, pauseResume, safeAbort,
                    emergencyRollback, restartRecovered, approvalSets,
                    finalized, eligible, operatorDecisionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RunCompletionCase ==
    /\ ~halted /\ registered /\ ~finalized /\ schedulePosition = 0
    /\ completedCases' = completedCases + 1
    /\ schedulePosition' = schedulePosition + 1
    /\ multiWindow' = TRUE /\ pauseResume' = TRUE /\ restartRecovered' = TRUE
    /\ approvalSets' = approvalSets + 1
    /\ UNCHANGED <<manifestValid, registered, approvalRenewal, approvalExpiry,
                    safeAbort, emergencyRollback, finalized, eligible,
                    operatorDecisionRequired, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

RunSafeAbortCase ==
    /\ ~halted /\ registered /\ ~finalized /\ schedulePosition = 1
    /\ completedCases' = completedCases + 1
    /\ schedulePosition' = schedulePosition + 1 /\ safeAbort' = TRUE
    /\ approvalSets' = approvalSets + 1 /\ approvalRenewal' = TRUE
    /\ UNCHANGED <<manifestValid, registered, multiWindow, approvalExpiry,
                    pauseResume, emergencyRollback, restartRecovered, finalized,
                    eligible, operatorDecisionRequired, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

RunRollbackCase ==
    /\ ~halted /\ registered /\ ~finalized /\ schedulePosition = 2
    /\ completedCases' = completedCases + 1
    /\ schedulePosition' = schedulePosition + 1 /\ emergencyRollback' = TRUE
    /\ approvalSets' = approvalSets + 1
    /\ UNCHANGED <<manifestValid, registered, multiWindow, approvalRenewal,
                    approvalExpiry, pauseResume, safeAbort, restartRecovered,
                    finalized, eligible, operatorDecisionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RunExpiryCase ==
    /\ ~halted /\ registered /\ ~finalized /\ schedulePosition = 3
    /\ completedCases' = completedCases + 1
    /\ schedulePosition' = schedulePosition + 1 /\ approvalExpiry' = TRUE
    /\ approvalSets' = approvalSets + 1
    /\ UNCHANGED <<manifestValid, registered, multiWindow, approvalRenewal,
                    pauseResume, safeAbort, emergencyRollback, restartRecovered,
                    finalized, eligible, operatorDecisionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ eligible' = (completedCases = CaseCount
                     /\ schedulePosition = CaseCount /\ AllCoverage
                     /\ approvalSets >= 2)
    /\ UNCHANGED <<manifestValid, registered, completedCases,
                    schedulePosition, multiWindow, approvalRenewal,
                    approvalExpiry, pauseResume, safeAbort, emergencyRollback,
                    restartRecovered, approvalSets, operatorDecisionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<manifestValid, registered, completedCases,
                    schedulePosition, multiWindow, approvalRenewal,
                    approvalExpiry, pauseResume, safeAbort, emergencyRollback,
                    restartRecovered, approvalSets, finalized, eligible,
                    operatorDecisionRequired, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars
Terminal == finalized /\ UNCHANGED vars

Next == ValidateManifest \/ Register \/ RunCompletionCase \/ RunSafeAbortCase
        \/ RunRollbackCase \/ RunExpiryCase \/ Finalize \/ IntegrityFailure
        \/ Halted \/ Terminal

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ manifestValid \in BOOLEAN /\ registered \in BOOLEAN
    /\ completedCases \in 0..CaseCount /\ schedulePosition \in 0..CaseCount
    /\ multiWindow \in BOOLEAN /\ approvalRenewal \in BOOLEAN
    /\ approvalExpiry \in BOOLEAN /\ pauseResume \in BOOLEAN
    /\ safeAbort \in BOOLEAN /\ emergencyRollback \in BOOLEAN
    /\ restartRecovered \in BOOLEAN /\ approvalSets \in 0..CaseCount
    /\ finalized \in BOOLEAN /\ eligible \in BOOLEAN
    /\ operatorDecisionRequired \in BOOLEAN /\ credentialCreated \in BOOLEAN
    /\ authenticationAuthority \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN /\ trafficAuthority \in BOOLEAN
    /\ cloudAuthority \in BOOLEAN /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegisteredRequiresManifest == registered => manifestValid
CasesStayOrdered == completedCases = schedulePosition
RestartRequiresCompletedCase == restartRecovered => completedCases >= 1
RenewalRequiresTwoApprovalSets == approvalRenewal => approvalSets >= 2
EligibilityRequiresCompleteCampaign ==
    eligible => (finalized /\ completedCases = CaseCount
                 /\ schedulePosition = CaseCount /\ AllCoverage
                 /\ approvalSets >= 2)
NoExternalAuthority ==
    /\ operatorDecisionRequired /\ ~credentialCreated
    /\ ~authenticationAuthority /\ ~deploymentAuthority /\ ~rollbackAuthority
    /\ ~trafficAuthority /\ ~cloudAuthority /\ ~liveTradingAuthority

=============================================================================
