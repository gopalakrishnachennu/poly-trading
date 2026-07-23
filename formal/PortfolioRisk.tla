--------------------------- MODULE PortfolioRisk ---------------------------
EXTENDS Integers, TLC

CONSTANTS WealthValues, ExposureValues, CapitalFloor, ExposureLimit

VARIABLES mode, reconciled, scenariosComplete, minimumWealth,
          grossExposure, conditionExposure, groupExposure

vars == <<mode, reconciled, scenariosComplete, minimumWealth,
          grossExposure, conditionExposure, groupExposure>>

Safe(r, complete, wealth, gross, condition, group) ==
    /\ r
    /\ complete
    /\ wealth >= CapitalFloor
    /\ gross <= ExposureLimit
    /\ condition <= ExposureLimit
    /\ group <= ExposureLimit

Init ==
    /\ mode = "Idle"
    /\ reconciled = FALSE
    /\ scenariosComplete = FALSE
    /\ minimumWealth = 0
    /\ grossExposure = 0
    /\ conditionExposure = 0
    /\ groupExposure = 0

Evaluate(r, complete, wealth, gross, condition, group) ==
    /\ mode \in {"Idle", "Approve", "NoTrade"}
    /\ r \in BOOLEAN
    /\ complete \in BOOLEAN
    /\ wealth \in WealthValues
    /\ gross \in ExposureValues
    /\ condition \in ExposureValues
    /\ group \in ExposureValues
    /\ reconciled' = r
    /\ scenariosComplete' = complete
    /\ minimumWealth' = wealth
    /\ grossExposure' = gross
    /\ conditionExposure' = condition
    /\ groupExposure' = group
    /\ mode' = IF Safe(r, complete, wealth, gross, condition, group)
                THEN "Approve" ELSE "NoTrade"

IntegrityFailure ==
    /\ mode \in {"Idle", "Approve", "NoTrade"}
    /\ mode' = "Halted"
    /\ UNCHANGED <<reconciled, scenariosComplete, minimumWealth,
                    grossExposure, conditionExposure, groupExposure>>

Halted ==
    /\ mode = "Halted"
    /\ UNCHANGED vars

Next ==
    \/ \E r \in BOOLEAN,
          complete \in BOOLEAN,
          wealth \in WealthValues,
          gross \in ExposureValues,
          condition \in ExposureValues,
          group \in ExposureValues:
          Evaluate(r, complete, wealth, gross, condition, group)
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ mode \in {"Idle", "Approve", "NoTrade", "Halted"}
    /\ reconciled \in BOOLEAN
    /\ scenariosComplete \in BOOLEAN
    /\ minimumWealth \in WealthValues \cup {0}
    /\ grossExposure \in ExposureValues \cup {0}
    /\ conditionExposure \in ExposureValues \cup {0}
    /\ groupExposure \in ExposureValues \cup {0}

ApproveImpliesReconciled == mode = "Approve" => reconciled
ApproveImpliesComplete == mode = "Approve" => scenariosComplete
ApprovePreservesFloor == mode = "Approve" => minimumWealth >= CapitalFloor
ApproveRespectsExposure ==
    mode = "Approve" =>
        /\ grossExposure <= ExposureLimit
        /\ conditionExposure <= ExposureLimit
        /\ groupExposure <= ExposureLimit
HaltIsAbsorbing == [](mode = "Halted" => [](mode = "Halted"))

=============================================================================
