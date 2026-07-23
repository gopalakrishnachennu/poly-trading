--------------------- MODULE CompleteSetArbitrage ---------------------
EXTENDS Naturals, TLC

ContextStates == {"Absent", "Degraded", "Ready"}

VARIABLES contextState, evaluationUsed, opportunityCount, lastOpportunity,
          intentCount, evaluatedLiquid, evaluatedProfitable,
          evaluatedThresholdsPass, riskApproved, capitalReserved,
          splitOrMerged, submitted, halted

vars == <<contextState, evaluationUsed, opportunityCount, lastOpportunity,
          intentCount, evaluatedLiquid, evaluatedProfitable,
          evaluatedThresholdsPass, riskApproved, capitalReserved,
          splitOrMerged, submitted, halted>>

Init ==
    /\ contextState = "Absent"
    /\ evaluationUsed = FALSE
    /\ opportunityCount = 0
    /\ lastOpportunity = FALSE
    /\ intentCount = 0
    /\ evaluatedLiquid = FALSE
    /\ evaluatedProfitable = FALSE
    /\ evaluatedThresholdsPass = FALSE
    /\ riskApproved = FALSE
    /\ capitalReserved = FALSE
    /\ splitOrMerged = FALSE
    /\ submitted = FALSE
    /\ halted = FALSE

Capture(state) ==
    /\ ~halted
    /\ state \in {"Degraded", "Ready"}
    /\ contextState' = state
    /\ lastOpportunity' = FALSE
    /\ intentCount' = 0
    /\ UNCHANGED <<evaluationUsed, opportunityCount, evaluatedLiquid,
                    evaluatedProfitable, evaluatedThresholdsPass, riskApproved,
                    capitalReserved, splitOrMerged, submitted, halted>>

Evaluate(liquid, profitable, thresholdsPass) ==
    /\ ~halted
    /\ contextState # "Absent"
    /\ ~evaluationUsed
    /\ liquid \in BOOLEAN
    /\ profitable \in BOOLEAN
    /\ thresholdsPass \in BOOLEAN
    /\ evaluationUsed' = TRUE
    /\ evaluatedLiquid' = liquid
    /\ evaluatedProfitable' = profitable
    /\ evaluatedThresholdsPass' = thresholdsPass
    /\ lastOpportunity' =
          (contextState = "Ready" /\ liquid /\ profitable /\ thresholdsPass)
    /\ IF lastOpportunity'
          THEN /\ opportunityCount' = opportunityCount + 1
               /\ intentCount' = 2
          ELSE /\ UNCHANGED opportunityCount
               /\ intentCount' = 0
    /\ UNCHANGED <<contextState, riskApproved, capitalReserved,
                    splitOrMerged, submitted, halted>>

RejectReplay ==
    /\ ~halted
    /\ evaluationUsed
    /\ lastOpportunity' = FALSE
    /\ intentCount' = 0
    /\ UNCHANGED <<contextState, evaluationUsed, opportunityCount,
                    evaluatedLiquid, evaluatedProfitable,
                    evaluatedThresholdsPass, riskApproved, capitalReserved,
                    splitOrMerged, submitted, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ lastOpportunity' = FALSE
    /\ intentCount' = 0
    /\ UNCHANGED <<contextState, evaluationUsed, opportunityCount,
                    evaluatedLiquid, evaluatedProfitable,
                    evaluatedThresholdsPass, riskApproved, capitalReserved,
                    splitOrMerged, submitted>>

Halted ==
    /\ halted
    /\ UNCHANGED vars

Next ==
    \/ \E state \in {"Degraded", "Ready"}: Capture(state)
    \/ \E liquid \in BOOLEAN, profitable \in BOOLEAN,
          thresholdsPass \in BOOLEAN:
          Evaluate(liquid, profitable, thresholdsPass)
    \/ RejectReplay
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ contextState \in ContextStates
    /\ evaluationUsed \in BOOLEAN
    /\ opportunityCount \in Nat
    /\ lastOpportunity \in BOOLEAN
    /\ intentCount \in 0..2
    /\ evaluatedLiquid \in BOOLEAN
    /\ evaluatedProfitable \in BOOLEAN
    /\ evaluatedThresholdsPass \in BOOLEAN
    /\ riskApproved \in BOOLEAN
    /\ capitalReserved \in BOOLEAN
    /\ splitOrMerged \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ halted \in BOOLEAN

OpportunityRequiresReady ==
    lastOpportunity => contextState = "Ready" /\ evaluationUsed
OpportunityRequiresEconomics ==
    lastOpportunity =>
        evaluatedLiquid /\ evaluatedProfitable /\ evaluatedThresholdsPass
OpportunityHasExactlyTwoLegs == lastOpportunity <=> intentCount = 2
EvaluationCannotReplay == opportunityCount <= 1
DetectorHasNoDownstreamAuthority ==
    ~riskApproved /\ ~capitalReserved /\ ~splitOrMerged /\ ~submitted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
