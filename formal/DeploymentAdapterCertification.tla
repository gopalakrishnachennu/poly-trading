---------------- MODULE DeploymentAdapterCertification ----------------
EXTENDS Naturals, TLC

VARIABLES reportsValid, contractSafe, registered, fixtureCoverage,
          privilegeCoverage, recoveryScenarioCoverage, recoveryRegionCoverage,
          finalized, certified, credentialCreated, authenticationAuthority,
          deploymentAuthority, rollbackAuthority, trafficAuthority,
          cloudAuthority, liveTradingAuthority, halted

vars == <<reportsValid, contractSafe, registered, fixtureCoverage,
          privilegeCoverage, recoveryScenarioCoverage, recoveryRegionCoverage,
          finalized, certified, credentialCreated, authenticationAuthority,
          deploymentAuthority, rollbackAuthority, trafficAuthority,
          cloudAuthority, liveTradingAuthority, halted>>

Init ==
    /\ reportsValid = FALSE /\ contractSafe = FALSE /\ registered = FALSE
    /\ fixtureCoverage = 0 /\ privilegeCoverage = 0
    /\ recoveryScenarioCoverage = 0 /\ recoveryRegionCoverage = 0
    /\ finalized = FALSE /\ certified = FALSE /\ credentialCreated = FALSE
    /\ authenticationAuthority = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ trafficAuthority = FALSE
    /\ cloudAuthority = FALSE /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidateReports ==
    /\ ~halted /\ ~registered /\ reportsValid' = TRUE
    /\ UNCHANGED <<contractSafe, registered, fixtureCoverage, privilegeCoverage,
                    recoveryScenarioCoverage, recoveryRegionCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ValidateContract ==
    /\ ~halted /\ ~registered /\ contractSafe' = TRUE
    /\ UNCHANGED <<reportsValid, registered, fixtureCoverage, privilegeCoverage,
                    recoveryScenarioCoverage, recoveryRegionCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Register ==
    /\ ~halted /\ ~registered /\ reportsValid /\ contractSafe
    /\ registered' = TRUE
    /\ UNCHANGED <<reportsValid, contractSafe, fixtureCoverage,
                    privilegeCoverage, recoveryScenarioCoverage,
                    recoveryRegionCoverage, finalized, certified,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RecordFixture ==
    /\ ~halted /\ registered /\ ~finalized /\ fixtureCoverage < 2
    /\ fixtureCoverage' = fixtureCoverage + 1
    /\ UNCHANGED <<reportsValid, contractSafe, registered, privilegeCoverage,
                    recoveryScenarioCoverage, recoveryRegionCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RecordPrivilege ==
    /\ ~halted /\ registered /\ ~finalized /\ privilegeCoverage < 2
    /\ privilegeCoverage' = privilegeCoverage + 1
    /\ UNCHANGED <<reportsValid, contractSafe, registered, fixtureCoverage,
                    recoveryScenarioCoverage, recoveryRegionCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RecordRecoveryScenario ==
    /\ ~halted /\ registered /\ ~finalized /\ recoveryScenarioCoverage < 2
    /\ recoveryScenarioCoverage' = recoveryScenarioCoverage + 1
    /\ UNCHANGED <<reportsValid, contractSafe, registered, fixtureCoverage,
                    privilegeCoverage, recoveryRegionCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

RecordRecoveryRegion ==
    /\ ~halted /\ registered /\ ~finalized /\ recoveryRegionCoverage < 2
    /\ recoveryRegionCoverage' = recoveryRegionCoverage + 1
    /\ UNCHANGED <<reportsValid, contractSafe, registered, fixtureCoverage,
                    privilegeCoverage, recoveryScenarioCoverage, finalized,
                    certified, credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ certified' = (fixtureCoverage = 2 /\ privilegeCoverage = 2
                     /\ recoveryScenarioCoverage = 2
                     /\ recoveryRegionCoverage = 2)
    /\ UNCHANGED <<reportsValid, contractSafe, registered, fixtureCoverage,
                    privilegeCoverage, recoveryScenarioCoverage,
                    recoveryRegionCoverage, credentialCreated,
                    authenticationAuthority, deploymentAuthority,
                    rollbackAuthority, trafficAuthority, cloudAuthority,
                    liveTradingAuthority, halted>>

UnsafeEvidence ==
    /\ ~halted /\ registered /\ ~finalized /\ halted' = TRUE
    /\ UNCHANGED <<reportsValid, contractSafe, registered, fixtureCoverage,
                    privilegeCoverage, recoveryScenarioCoverage,
                    recoveryRegionCoverage, finalized, certified,
                    credentialCreated, authenticationAuthority,
                    deploymentAuthority, rollbackAuthority, trafficAuthority,
                    cloudAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars
Terminal == finalized /\ UNCHANGED vars

Next == ValidateReports \/ ValidateContract \/ Register \/ RecordFixture
        \/ RecordPrivilege \/ RecordRecoveryScenario \/ RecordRecoveryRegion
        \/ Finalize \/ UnsafeEvidence \/ Halted \/ Terminal

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ reportsValid \in BOOLEAN /\ contractSafe \in BOOLEAN
    /\ registered \in BOOLEAN /\ fixtureCoverage \in 0..2
    /\ privilegeCoverage \in 0..2 /\ recoveryScenarioCoverage \in 0..2
    /\ recoveryRegionCoverage \in 0..2 /\ finalized \in BOOLEAN
    /\ certified \in BOOLEAN /\ credentialCreated \in BOOLEAN
    /\ authenticationAuthority \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ rollbackAuthority \in BOOLEAN /\ trafficAuthority \in BOOLEAN
    /\ cloudAuthority \in BOOLEAN /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegisteredRequiresSafeSubject == registered => (reportsValid /\ contractSafe)
CertifiedRequiresCompleteEvidence ==
    certified => (finalized /\ fixtureCoverage = 2 /\ privilegeCoverage = 2
                  /\ recoveryScenarioCoverage = 2 /\ recoveryRegionCoverage = 2)
NoAuthority ==
    /\ ~credentialCreated /\ ~authenticationAuthority /\ ~deploymentAuthority
    /\ ~rollbackAuthority /\ ~trafficAuthority /\ ~cloudAuthority
    /\ ~liveTradingAuthority

=============================================================================
