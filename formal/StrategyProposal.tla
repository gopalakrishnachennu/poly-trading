------------------------ MODULE StrategyProposal ------------------------
EXTENDS Naturals, TLC

ContextStates == {"Absent", "Degraded", "Ready"}

VARIABLES contextState, exactFrame, proposalUsed, candidateCount,
          lastCandidate, riskApproved, capitalReserved, policyPermitted,
          submitted, halted

vars == <<contextState, exactFrame, proposalUsed, candidateCount,
          lastCandidate, riskApproved, capitalReserved, policyPermitted,
          submitted, halted>>

Init ==
    /\ contextState = "Absent"
    /\ exactFrame = FALSE
    /\ proposalUsed = FALSE
    /\ candidateCount = 0
    /\ lastCandidate = FALSE
    /\ riskApproved = FALSE
    /\ capitalReserved = FALSE
    /\ policyPermitted = FALSE
    /\ submitted = FALSE
    /\ halted = FALSE

CaptureDegraded ==
    /\ ~halted
    /\ contextState' = "Degraded"
    /\ exactFrame' = TRUE
    /\ lastCandidate' = FALSE
    /\ UNCHANGED <<proposalUsed, candidateCount, riskApproved,
                    capitalReserved, policyPermitted, submitted, halted>>

CaptureReady ==
    /\ ~halted
    /\ contextState' = "Ready"
    /\ exactFrame' = TRUE
    /\ lastCandidate' = FALSE
    /\ UNCHANGED <<proposalUsed, candidateCount, riskApproved,
                    capitalReserved, policyPermitted, submitted, halted>>

EvaluateProposal ==
    /\ ~halted
    /\ contextState # "Absent"
    /\ ~proposalUsed
    /\ proposalUsed' = TRUE
    /\ lastCandidate' = (contextState = "Ready" /\ exactFrame)
    /\ IF lastCandidate'
          THEN candidateCount' = candidateCount + 1
          ELSE UNCHANGED candidateCount
    /\ UNCHANGED <<contextState, exactFrame, riskApproved, capitalReserved,
                    policyPermitted, submitted, halted>>

RejectReplay ==
    /\ ~halted
    /\ proposalUsed
    /\ lastCandidate' = FALSE
    /\ UNCHANGED <<contextState, exactFrame, proposalUsed, candidateCount,
                    riskApproved, capitalReserved, policyPermitted, submitted,
                    halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ lastCandidate' = FALSE
    /\ UNCHANGED <<contextState, exactFrame, proposalUsed, candidateCount,
                    riskApproved, capitalReserved, policyPermitted, submitted>>

Halted ==
    /\ halted
    /\ UNCHANGED vars

Next ==
    \/ CaptureDegraded
    \/ CaptureReady
    \/ EvaluateProposal
    \/ RejectReplay
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ contextState \in ContextStates
    /\ exactFrame \in BOOLEAN
    /\ proposalUsed \in BOOLEAN
    /\ candidateCount \in Nat
    /\ lastCandidate \in BOOLEAN
    /\ riskApproved \in BOOLEAN
    /\ capitalReserved \in BOOLEAN
    /\ policyPermitted \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ halted \in BOOLEAN

CandidateRequiresReadyContext ==
    lastCandidate => contextState = "Ready" /\ exactFrame /\ proposalUsed
ProposalCannotReplay == candidateCount <= 1
ProposalHasNoDownstreamAuthority ==
    ~riskApproved /\ ~capitalReserved /\ ~policyPermitted /\ ~submitted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
