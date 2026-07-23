-------------------- MODULE ProductionChangeReadiness --------------------
EXTENDS Naturals, TLC

VARIABLES evidenceValid, subjectBound, registered, evidenceComplete,
          diversityComplete, regressionComplete, releaseApproved, riskApproved,
          operationsApproved, operatorsDistinct, finalized, ready,
          operatorExecutionRequired, credentialCreated, authenticationAuthority,
          deploymentAuthority, rollbackAuthority, trafficAuthority,
          cloudAuthority, liveTradingAuthority, halted

vars == <<evidenceValid, subjectBound, registered, evidenceComplete,
          diversityComplete, regressionComplete, releaseApproved, riskApproved,
          operationsApproved, operatorsDistinct, finalized, ready,
          operatorExecutionRequired, credentialCreated, authenticationAuthority,
          deploymentAuthority, rollbackAuthority, trafficAuthority,
          cloudAuthority, liveTradingAuthority, halted>>

Init ==
    /\ evidenceValid = FALSE /\ subjectBound = FALSE /\ registered = FALSE
    /\ evidenceComplete = FALSE /\ diversityComplete = FALSE
    /\ regressionComplete = FALSE /\ releaseApproved = FALSE
    /\ riskApproved = FALSE /\ operationsApproved = FALSE
    /\ operatorsDistinct = FALSE /\ finalized = FALSE /\ ready = FALSE
    /\ operatorExecutionRequired = TRUE /\ credentialCreated = FALSE
    /\ authenticationAuthority = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ trafficAuthority = FALSE
    /\ cloudAuthority = FALSE /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidateEvidence ==
    /\ ~halted /\ ~registered /\ evidenceValid' = TRUE
    /\ UNCHANGED <<subjectBound, registered, evidenceComplete,
                    diversityComplete, regressionComplete, releaseApproved,
                    riskApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

BindSubject ==
    /\ ~halted /\ ~registered /\ subjectBound' = TRUE
    /\ UNCHANGED <<evidenceValid, registered, evidenceComplete,
                    diversityComplete, regressionComplete, releaseApproved,
                    riskApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Register ==
    /\ ~halted /\ ~registered /\ evidenceValid /\ subjectBound
    /\ registered' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, evidenceComplete,
                    diversityComplete, regressionComplete, releaseApproved,
                    riskApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

AggregateEvidence ==
    /\ ~halted /\ registered /\ ~finalized
    /\ evidenceComplete' = TRUE /\ diversityComplete' = TRUE
    /\ regressionComplete' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, registered, releaseApproved,
                    riskApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveRelease ==
    /\ ~halted /\ registered /\ ~finalized /\ releaseApproved' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, registered,
                    evidenceComplete, diversityComplete, regressionComplete,
                    riskApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveRisk ==
    /\ ~halted /\ registered /\ ~finalized /\ releaseApproved
    /\ riskApproved' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, registered,
                    evidenceComplete, diversityComplete, regressionComplete,
                    releaseApproved, operationsApproved, operatorsDistinct,
                    finalized, ready, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveOperationsDistinct ==
    /\ ~halted /\ registered /\ ~finalized
    /\ releaseApproved /\ riskApproved
    /\ operationsApproved' = TRUE /\ operatorsDistinct' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, registered,
                    evidenceComplete, diversityComplete, regressionComplete,
                    releaseApproved, riskApproved, finalized, ready,
                    operatorExecutionRequired, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized /\ finalized' = TRUE
    /\ ready' = (evidenceComplete /\ diversityComplete /\ regressionComplete
                  /\ releaseApproved /\ riskApproved /\ operationsApproved
                  /\ operatorsDistinct)
    /\ UNCHANGED <<evidenceValid, subjectBound, registered,
                    evidenceComplete, diversityComplete, regressionComplete,
                    releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, operatorExecutionRequired,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<evidenceValid, subjectBound, registered,
                    evidenceComplete, diversityComplete, regressionComplete,
                    releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready,
                    operatorExecutionRequired, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars
Terminal == finalized /\ UNCHANGED vars

Next == ValidateEvidence \/ BindSubject \/ Register \/ AggregateEvidence
        \/ ApproveRelease \/ ApproveRisk \/ ApproveOperationsDistinct
        \/ Finalize \/ IntegrityFailure \/ Halted \/ Terminal

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ evidenceValid \in BOOLEAN /\ subjectBound \in BOOLEAN
    /\ registered \in BOOLEAN /\ evidenceComplete \in BOOLEAN
    /\ diversityComplete \in BOOLEAN /\ regressionComplete \in BOOLEAN
    /\ releaseApproved \in BOOLEAN /\ riskApproved \in BOOLEAN
    /\ operationsApproved \in BOOLEAN /\ operatorsDistinct \in BOOLEAN
    /\ finalized \in BOOLEAN /\ ready \in BOOLEAN
    /\ operatorExecutionRequired \in BOOLEAN /\ credentialCreated \in BOOLEAN
    /\ authenticationAuthority \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN /\ trafficAuthority \in BOOLEAN
    /\ cloudAuthority \in BOOLEAN /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegisteredRequiresSafeSubject == registered => (evidenceValid /\ subjectBound)
ReadyRequiresEveryGate ==
    ready => (finalized /\ evidenceComplete /\ diversityComplete
              /\ regressionComplete /\ releaseApproved /\ riskApproved
              /\ operationsApproved /\ operatorsDistinct)
NoExternalAuthority ==
    /\ operatorExecutionRequired /\ ~credentialCreated
    /\ ~authenticationAuthority /\ ~deploymentAuthority /\ ~rollbackAuthority
    /\ ~trafficAuthority /\ ~cloudAuthority /\ ~liveTradingAuthority

=============================================================================
