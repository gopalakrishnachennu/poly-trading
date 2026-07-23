-------------------- MODULE PairedPaperExecution --------------------
EXTENDS Naturals, TLC

States == {"None", "Authorized", "Submitted", "Delayed", "Live",
           "Partial", "Unknown", "CancelPending", "Matched", "NoFill"}

Fillable(state) == state \in {"Submitted", "Delayed", "Live", "Partial",
                              "Unknown", "CancelPending"}
Exposure(state) == Fillable(state) \/ state = "Matched"

VARIABLES staged, reservations, firstPermit, firstPermitUsed, firstState,
          hedgePermit, hedgePermitUsed, hedgeState, fills, handoffs,
          realSubmission, signing, childHalted, halted

vars == <<staged, reservations, firstPermit, firstPermitUsed, firstState,
          hedgePermit, hedgePermitUsed, hedgeState, fills, handoffs,
          realSubmission, signing, childHalted, halted>>

Init ==
    /\ staged = FALSE
    /\ reservations = 0
    /\ firstPermit = FALSE
    /\ firstPermitUsed = FALSE
    /\ firstState = "None"
    /\ hedgePermit = FALSE
    /\ hedgePermitUsed = FALSE
    /\ hedgeState = "None"
    /\ fills = 0
    /\ handoffs = 0
    /\ realSubmission = FALSE
    /\ signing = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

Stage ==
    /\ ~halted /\ ~staged
    /\ staged' = TRUE /\ reservations' = 2
    /\ UNCHANGED <<firstPermit, firstPermitUsed, firstState, hedgePermit,
                    hedgePermitUsed, hedgeState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

AuthorizeFirst ==
    /\ ~halted /\ staged /\ reservations = 2 /\ ~firstPermit
    /\ firstPermit' = TRUE /\ firstState' = "Authorized"
    /\ UNCHANGED <<staged, reservations, firstPermitUsed, hedgePermit,
                    hedgePermitUsed, hedgeState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

SubmitFirst ==
    /\ ~halted /\ reservations = 2 /\ firstPermit /\ ~firstPermitUsed
    /\ firstPermitUsed' = TRUE /\ firstState' = "Submitted"
    /\ UNCHANGED <<staged, reservations, firstPermit, hedgePermit,
                    hedgePermitUsed, hedgeState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

AdvanceFirst(nextState) ==
    /\ ~halted /\ reservations = 2
    /\ firstState \in {"Submitted", "Delayed", "Live", "Partial", "Unknown", "CancelPending"}
    /\ nextState \in {"Delayed", "Live", "Partial", "Unknown", "CancelPending", "NoFill"}
    /\ (firstState = "Partial" => nextState # "NoFill")
    /\ firstState' = nextState
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    hedgePermit, hedgePermitUsed, hedgeState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

FillFirst(full) ==
    /\ ~halted /\ reservations = 2 /\ fills < 2 /\ Fillable(firstState) /\ full \in BOOLEAN
    /\ firstState' = IF full THEN "Matched" ELSE "Partial"
    /\ fills' = fills + 1 /\ handoffs' = handoffs + 1
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    hedgePermit, hedgePermitUsed, hedgeState,
                    realSubmission, signing, childHalted, halted>>

AuthorizeHedge ==
    /\ ~halted /\ reservations = 2 /\ firstState = "Matched" /\ ~hedgePermit
    /\ hedgePermit' = TRUE /\ hedgeState' = "Authorized"
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    hedgePermitUsed, firstState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

SubmitHedge ==
    /\ ~halted /\ reservations = 2 /\ hedgePermit /\ ~hedgePermitUsed
    /\ hedgePermitUsed' = TRUE /\ hedgeState' = "Submitted"
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    firstState, hedgePermit, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

FillHedge(full) ==
    /\ ~halted /\ reservations = 2 /\ fills < 2 /\ Fillable(hedgeState) /\ full \in BOOLEAN
    /\ hedgeState' = IF full THEN "Matched" ELSE "Partial"
    /\ fills' = fills + 1 /\ handoffs' = handoffs + 1
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    firstState, hedgePermit, hedgePermitUsed,
                    realSubmission, signing, childHalted, halted>>

AbortSafe ==
    /\ ~halted /\ staged
    /\ firstState \in {"None", "NoFill"}
    /\ hedgeState \in {"None", "NoFill"}
    /\ reservations' = 0
    /\ UNCHANGED <<staged, firstPermit, firstPermitUsed, firstState,
                    hedgePermit, hedgePermitUsed, hedgeState, fills, handoffs,
                    realSubmission, signing, childHalted, halted>>

ChildFailure ==
    /\ ~halted /\ childHalted' = TRUE /\ halted' = TRUE
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    firstState, hedgePermit, hedgePermitUsed, hedgeState,
                    fills, handoffs, realSubmission, signing>>

IntegrityFailure ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<staged, reservations, firstPermit, firstPermitUsed,
                    firstState, hedgePermit, hedgePermitUsed, hedgeState,
                    fills, handoffs, realSubmission, signing, childHalted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ Stage \/ AuthorizeFirst \/ SubmitFirst
    \/ \E nextState \in States: AdvanceFirst(nextState)
    \/ \E full \in BOOLEAN: FillFirst(full)
    \/ AuthorizeHedge \/ SubmitHedge
    \/ \E full \in BOOLEAN: FillHedge(full)
    \/ AbortSafe \/ ChildFailure \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ staged \in BOOLEAN /\ reservations \in {0, 2}
    /\ firstPermit \in BOOLEAN /\ firstPermitUsed \in BOOLEAN /\ firstState \in States
    /\ hedgePermit \in BOOLEAN /\ hedgePermitUsed \in BOOLEAN /\ hedgeState \in States
    /\ fills \in 0..2 /\ handoffs \in 0..2
    /\ realSubmission \in BOOLEAN /\ signing \in BOOLEAN
    /\ childHalted \in BOOLEAN /\ halted \in BOOLEAN

PermitSingleUse == firstPermitUsed => firstPermit
HedgeRequiresFirstMatch == hedgePermit => firstState = "Matched"
HandoffEqualsFills == handoffs = fills
ExposureRetainsPair == (Exposure(firstState) \/ Exposure(hedgeState)) => reservations = 2
NoLiveAuthority == ~realSubmission /\ ~signing
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
