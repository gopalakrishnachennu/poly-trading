---------------- MODULE PromotionGovernance ----------------
EXTENDS Naturals, TLC

VARIABLES registered, evidenceComplete, diverse, regressionPassed,
          artifactsBound, rollbackBound, riskDecision, releaseDecision,
          riskOperator, releaseOperator, finalized, finalCount, eligible,
          operatorExecutionRequired, canaryExecutionAuthority,
          promotionAuthority, deploymentAuthority, credentialAuthority,
          liveTradingAuthority, halted

vars == <<registered, evidenceComplete, diverse, regressionPassed,
          artifactsBound, rollbackBound, riskDecision, releaseDecision,
          riskOperator, releaseOperator, finalized, finalCount, eligible,
          operatorExecutionRequired, canaryExecutionAuthority,
          promotionAuthority, deploymentAuthority, credentialAuthority,
          liveTradingAuthority, halted>>

Init ==
    /\ registered = FALSE
    /\ evidenceComplete = FALSE
    /\ diverse = FALSE
    /\ regressionPassed = FALSE
    /\ artifactsBound = FALSE
    /\ rollbackBound = FALSE
    /\ riskDecision = 0
    /\ releaseDecision = 0
    /\ riskOperator = 0
    /\ releaseOperator = 0
    /\ finalized = FALSE
    /\ finalCount = 0
    /\ eligible = FALSE
    /\ operatorExecutionRequired = FALSE
    /\ canaryExecutionAuthority = FALSE
    /\ promotionAuthority = FALSE
    /\ deploymentAuthority = FALSE
    /\ credentialAuthority = FALSE
    /\ liveTradingAuthority = FALSE
    /\ halted = FALSE

Register(complete, independent, regression, artifacts, rollback) ==
    /\ ~halted /\ ~registered
    /\ complete \in BOOLEAN
    /\ independent \in BOOLEAN
    /\ regression \in BOOLEAN
    /\ artifacts \in BOOLEAN
    /\ rollback \in BOOLEAN
    /\ registered' = TRUE
    /\ evidenceComplete' = complete
    /\ diverse' = independent
    /\ regressionPassed' = regression
    /\ artifactsBound' = artifacts
    /\ rollbackBound' = rollback
    /\ UNCHANGED <<riskDecision, releaseDecision, riskOperator,
                    releaseOperator, finalized, finalCount, eligible,
                    operatorExecutionRequired, canaryExecutionAuthority,
                    promotionAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

RiskDecision(verdict, operator) ==
    /\ ~halted /\ registered /\ ~finalized /\ riskDecision = 0
    /\ verdict \in {1, 2}
    /\ operator \in {1, 2}
    /\ riskDecision' = verdict
    /\ riskOperator' = operator
    /\ UNCHANGED <<registered, evidenceComplete, diverse, regressionPassed,
                    artifactsBound, rollbackBound, releaseDecision,
                    releaseOperator, finalized, finalCount, eligible,
                    operatorExecutionRequired, canaryExecutionAuthority,
                    promotionAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

ReleaseDecision(verdict, operator) ==
    /\ ~halted /\ registered /\ ~finalized /\ releaseDecision = 0
    /\ verdict \in {1, 2}
    /\ operator \in {1, 2}
    /\ releaseDecision' = verdict
    /\ releaseOperator' = operator
    /\ UNCHANGED <<registered, evidenceComplete, diverse, regressionPassed,
                    artifactsBound, rollbackBound, riskDecision, riskOperator,
                    finalized, finalCount, eligible,
                    operatorExecutionRequired, canaryExecutionAuthority,
                    promotionAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ finalCount' = finalCount + 1
    /\ eligible' = (evidenceComplete /\ diverse /\ regressionPassed
                     /\ artifactsBound /\ rollbackBound
                     /\ riskDecision = 1 /\ releaseDecision = 1
                     /\ riskOperator # releaseOperator)
    /\ operatorExecutionRequired' = TRUE
    /\ UNCHANGED <<registered, evidenceComplete, diverse, regressionPassed,
                    artifactsBound, rollbackBound, riskDecision,
                    releaseDecision, riskOperator, releaseOperator,
                    canaryExecutionAuthority, promotionAuthority,
                    deploymentAuthority, credentialAuthority,
                    liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<registered, evidenceComplete, diverse, regressionPassed,
                    artifactsBound, rollbackBound, riskDecision,
                    releaseDecision, riskOperator, releaseOperator,
                    finalized, finalCount, eligible,
                    operatorExecutionRequired, canaryExecutionAuthority,
                    promotionAuthority, deploymentAuthority,
                    credentialAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ \E complete, independent, regression, artifacts, rollback \in BOOLEAN:
          Register(complete, independent, regression, artifacts, rollback)
    \/ \E verdict \in {1, 2}, operator \in {1, 2}:
          RiskDecision(verdict, operator)
    \/ \E verdict \in {1, 2}, operator \in {1, 2}:
          ReleaseDecision(verdict, operator)
    \/ Finalize
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ registered \in BOOLEAN
    /\ evidenceComplete \in BOOLEAN
    /\ diverse \in BOOLEAN
    /\ regressionPassed \in BOOLEAN
    /\ artifactsBound \in BOOLEAN
    /\ rollbackBound \in BOOLEAN
    /\ riskDecision \in 0..2
    /\ releaseDecision \in 0..2
    /\ riskOperator \in 0..2
    /\ releaseOperator \in 0..2
    /\ finalized \in BOOLEAN
    /\ finalCount \in 0..1
    /\ eligible \in BOOLEAN
    /\ operatorExecutionRequired \in BOOLEAN
    /\ canaryExecutionAuthority \in BOOLEAN
    /\ promotionAuthority \in BOOLEAN
    /\ deploymentAuthority \in BOOLEAN
    /\ credentialAuthority \in BOOLEAN
    /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

EligibleRequiresEvidence ==
    eligible => (registered /\ finalized /\ evidenceComplete /\ diverse
                 /\ regressionPassed /\ artifactsBound /\ rollbackBound)

EligibleRequiresDualControl ==
    eligible => (riskDecision = 1 /\ releaseDecision = 1
                 /\ riskOperator # releaseOperator)

FinalizationIsSingle == finalCount <= 1

FinalRequiresOperatorExecution == finalized => operatorExecutionRequired

NoAuthority ==
    /\ ~canaryExecutionAuthority
    /\ ~promotionAuthority
    /\ ~deploymentAuthority
    /\ ~credentialAuthority
    /\ ~liveTradingAuthority

=============================================================================
