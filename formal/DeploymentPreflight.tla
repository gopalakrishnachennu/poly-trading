---------------- MODULE DeploymentPreflight ----------------
EXTENDS Naturals, TLC

VARIABLES regionsValid, privilegeSafe, rollbackValid, fleetCurrent, packageActive,
          registered, releaseApproved, riskApproved, operationsApproved,
          operatorsDistinct, finalized, ready, credentialCreated,
          signingAuthority, deploymentAuthority, rollbackAuthority,
          cloudAuthority, liveTradingAuthority, halted

vars == <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent, packageActive,
          registered, releaseApproved, riskApproved, operationsApproved,
          operatorsDistinct, finalized, ready, credentialCreated,
          signingAuthority, deploymentAuthority, rollbackAuthority,
          cloudAuthority, liveTradingAuthority, halted>>

Init ==
    /\ regionsValid = FALSE /\ privilegeSafe = FALSE /\ rollbackValid = FALSE
    /\ fleetCurrent = FALSE /\ packageActive = FALSE /\ registered = FALSE
    /\ releaseApproved = FALSE /\ riskApproved = FALSE
    /\ operationsApproved = FALSE /\ operatorsDistinct = FALSE
    /\ finalized = FALSE /\ ready = FALSE /\ credentialCreated = FALSE
    /\ signingAuthority = FALSE /\ deploymentAuthority = FALSE
    /\ rollbackAuthority = FALSE /\ cloudAuthority = FALSE
    /\ liveTradingAuthority = FALSE /\ halted = FALSE

ValidateRegions ==
    /\ ~halted /\ ~registered /\ regionsValid' = TRUE
    /\ UNCHANGED <<privilegeSafe, rollbackValid, fleetCurrent, packageActive,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ValidatePrivilege ==
    /\ ~halted /\ ~registered /\ privilegeSafe' = TRUE
    /\ UNCHANGED <<regionsValid, rollbackValid, fleetCurrent, packageActive,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ValidateRollback ==
    /\ ~halted /\ ~registered /\ rollbackValid' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, fleetCurrent, packageActive,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ObserveFleet ==
    /\ ~halted /\ ~finalized /\ fleetCurrent' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, packageActive,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

InvalidateFleet ==
    /\ ~halted /\ registered /\ ~finalized /\ fleetCurrent
    /\ fleetCurrent' = FALSE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, packageActive,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ActivatePackage ==
    /\ ~halted /\ ~registered /\ packageActive' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    registered, releaseApproved, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Register ==
    /\ ~halted /\ ~registered /\ regionsValid /\ privilegeSafe
    /\ rollbackValid /\ fleetCurrent /\ packageActive
    /\ registered' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, releaseApproved, riskApproved,
                    operationsApproved, operatorsDistinct, finalized, ready,
                    credentialCreated, signingAuthority, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority, halted>>

ApproveRelease ==
    /\ ~halted /\ registered /\ ~finalized /\ releaseApproved' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, riskApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveRisk ==
    /\ ~halted /\ registered /\ ~finalized /\ riskApproved' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, releaseApproved, operationsApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

ApproveOperations ==
    /\ ~halted /\ registered /\ ~finalized /\ operationsApproved' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, releaseApproved, riskApproved,
                    operatorsDistinct, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

DistinctOperators ==
    /\ ~halted /\ registered /\ ~finalized /\ operatorsDistinct' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, releaseApproved, riskApproved,
                    operationsApproved, finalized, ready, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

Finalize ==
    /\ ~halted /\ registered /\ ~finalized
    /\ finalized' = TRUE
    /\ ready' = regionsValid /\ privilegeSafe /\ rollbackValid /\ fleetCurrent
                 /\ packageActive /\ releaseApproved /\ riskApproved
                 /\ operationsApproved /\ operatorsDistinct
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, releaseApproved, riskApproved,
                    operationsApproved, operatorsDistinct, credentialCreated,
                    signingAuthority, deploymentAuthority, rollbackAuthority,
                    cloudAuthority, liveTradingAuthority, halted>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<regionsValid, privilegeSafe, rollbackValid, fleetCurrent,
                    packageActive, registered, releaseApproved, riskApproved,
                    operationsApproved, operatorsDistinct, finalized, ready,
                    credentialCreated, signingAuthority, deploymentAuthority,
                    rollbackAuthority, cloudAuthority, liveTradingAuthority>>

Halted == halted /\ UNCHANGED vars

Next == ValidateRegions \/ ValidatePrivilege \/ ValidateRollback \/ ObserveFleet
        \/ InvalidateFleet \/ ActivatePackage \/ Register \/ ApproveRelease
        \/ ApproveRisk \/ ApproveOperations \/ DistinctOperators \/ Finalize
        \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ regionsValid \in BOOLEAN /\ privilegeSafe \in BOOLEAN
    /\ rollbackValid \in BOOLEAN /\ fleetCurrent \in BOOLEAN
    /\ packageActive \in BOOLEAN /\ registered \in BOOLEAN
    /\ releaseApproved \in BOOLEAN /\ riskApproved \in BOOLEAN
    /\ operationsApproved \in BOOLEAN /\ operatorsDistinct \in BOOLEAN
    /\ finalized \in BOOLEAN /\ ready \in BOOLEAN
    /\ credentialCreated \in BOOLEAN /\ signingAuthority \in BOOLEAN
    /\ deploymentAuthority \in BOOLEAN /\ rollbackAuthority \in BOOLEAN
    /\ cloudAuthority \in BOOLEAN /\ liveTradingAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegisteredRequiresPackage ==
    registered => (regionsValid /\ privilegeSafe /\ rollbackValid /\ packageActive)

ReadyRequiresAllGates ==
    ready => (finalized /\ registered /\ fleetCurrent /\ releaseApproved
              /\ riskApproved /\ operationsApproved /\ operatorsDistinct)

NoAuthority ==
    /\ ~credentialCreated /\ ~signingAuthority /\ ~deploymentAuthority
    /\ ~rollbackAuthority /\ ~cloudAuthority /\ ~liveTradingAuthority

=============================================================================
