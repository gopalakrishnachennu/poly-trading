------------------- MODULE PairedOpportunityRuntime -------------------
EXTENDS Naturals, TLC

Stages == {"Idle", "Detected", "Proposed", "RiskEvaluated", "NoTrade"}

VARIABLES stage, evaluationUsed, proposalCount, combinedRiskEvaluated,
          independentFillsCovered, pairDigestDistinct, riskEligible,
          singleLegAuthorized, reserved, submitted, childHalted, halted

vars == <<stage, evaluationUsed, proposalCount, combinedRiskEvaluated,
          independentFillsCovered, pairDigestDistinct, riskEligible,
          singleLegAuthorized, reserved, submitted, childHalted, halted>>

Init ==
    /\ stage = "Idle"
    /\ evaluationUsed = FALSE
    /\ proposalCount = 0
    /\ combinedRiskEvaluated = FALSE
    /\ independentFillsCovered = FALSE
    /\ pairDigestDistinct = FALSE
    /\ riskEligible = FALSE
    /\ singleLegAuthorized = FALSE
    /\ reserved = FALSE
    /\ submitted = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

Detect(opportunity) ==
    /\ ~halted
    /\ stage = "Idle"
    /\ ~evaluationUsed
    /\ opportunity \in BOOLEAN
    /\ evaluationUsed' = TRUE
    /\ stage' = IF opportunity THEN "Detected" ELSE "NoTrade"
    /\ UNCHANGED <<proposalCount, combinedRiskEvaluated,
                    independentFillsCovered, pairDigestDistinct, riskEligible,
                    singleLegAuthorized, reserved, submitted, childHalted,
                    halted>>

ValidateProposals ==
    /\ ~halted
    /\ stage = "Detected"
    /\ stage' = "Proposed"
    /\ proposalCount' = 2
    /\ UNCHANGED <<evaluationUsed, combinedRiskEvaluated,
                    independentFillsCovered, pairDigestDistinct, riskEligible,
                    singleLegAuthorized, reserved, submitted, childHalted,
                    halted>>

EvaluateCombinedRisk(pass) ==
    /\ ~halted
    /\ stage = "Proposed"
    /\ pass \in BOOLEAN
    /\ stage' = "RiskEvaluated"
    /\ combinedRiskEvaluated' = TRUE
    /\ independentFillsCovered' = TRUE
    /\ pairDigestDistinct' = TRUE
    /\ riskEligible' = pass
    /\ UNCHANGED <<evaluationUsed, proposalCount, singleLegAuthorized,
                    reserved, submitted, childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<stage, evaluationUsed, proposalCount,
                    combinedRiskEvaluated, independentFillsCovered,
                    pairDigestDistinct, riskEligible, singleLegAuthorized,
                    reserved, submitted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<stage, evaluationUsed, proposalCount,
                    combinedRiskEvaluated, independentFillsCovered,
                    pairDigestDistinct, riskEligible, singleLegAuthorized,
                    reserved, submitted, childHalted>>

Terminal ==
    /\ stage \in {"RiskEvaluated", "NoTrade"}
    /\ UNCHANGED vars

Halted ==
    /\ halted
    /\ UNCHANGED vars

Next ==
    \/ \E opportunity \in BOOLEAN: Detect(opportunity)
    \/ ValidateProposals
    \/ \E pass \in BOOLEAN: EvaluateCombinedRisk(pass)
    \/ ChildFailure
    \/ IntegrityFailure
    \/ Terminal
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ stage \in Stages
    /\ evaluationUsed \in BOOLEAN
    /\ proposalCount \in 0..2
    /\ combinedRiskEvaluated \in BOOLEAN
    /\ independentFillsCovered \in BOOLEAN
    /\ pairDigestDistinct \in BOOLEAN
    /\ riskEligible \in BOOLEAN
    /\ singleLegAuthorized \in BOOLEAN
    /\ reserved \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

RiskRequiresTwoProposals == combinedRiskEvaluated => proposalCount = 2
EligibilityRequiresCombinedScenarios ==
    riskEligible => combinedRiskEvaluated /\ independentFillsCovered
PairCannotAuthorizeSingleLeg ==
    pairDigestDistinct => ~singleLegAuthorized
RuntimeHasNoExecutionAuthority == ~reserved /\ ~submitted
ChildHaltPropagates == childHalted => halted
EvaluationCannotReplay == evaluationUsed => stage # "Idle"
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
