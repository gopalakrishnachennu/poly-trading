-------------------- MODULE PairedPlacementPolicy --------------------
EXTENDS Naturals, TLC

LegStates == {"None", "Authorized", "Expired", "Active", "Partial",
              "PartialTerminal", "Unknown", "Matched", "NoFill"}

VARIABLES staged, reservations, firstState, hedgeState, firstPermit,
          hedgePermit, firstEverPartial, hedgeEverPartial, aborted, signing,
          submitted, childHalted, halted

vars == <<staged, reservations, firstState, hedgeState, firstPermit,
          hedgePermit, firstEverPartial, hedgeEverPartial, aborted, signing,
          submitted, childHalted, halted>>

PossibleExposure(state) == state \in
    {"Active", "Partial", "PartialTerminal", "Unknown", "Matched"}
Safe(state) == state \in {"None", "Expired", "NoFill"}

Init ==
    /\ staged = FALSE
    /\ reservations = 0
    /\ firstState = "None"
    /\ hedgeState = "None"
    /\ firstPermit = FALSE
    /\ hedgePermit = FALSE
    /\ firstEverPartial = FALSE
    /\ hedgeEverPartial = FALSE
    /\ aborted = FALSE
    /\ signing = FALSE
    /\ submitted = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

Stage ==
    /\ ~halted
    /\ ~staged
    /\ staged' = TRUE
    /\ reservations' = 2
    /\ UNCHANGED <<firstState, hedgeState, firstPermit, hedgePermit,
                    firstEverPartial, hedgeEverPartial, aborted, signing,
                    submitted, childHalted, halted>>

AuthorizeFirst ==
    /\ ~halted
    /\ staged
    /\ ~aborted
    /\ firstState = "None"
    /\ firstPermit' = TRUE
    /\ firstState' = "Authorized"
    /\ UNCHANGED <<staged, reservations, hedgeState, hedgePermit, aborted,
                    firstEverPartial, hedgeEverPartial, signing, submitted,
                    childHalted, halted>>

AdvanceFirst(nextState) ==
    /\ ~halted
    /\ firstState \in {"Authorized", "Active", "Partial", "Unknown"}
    /\ nextState \in {"Active", "Partial", "PartialTerminal", "Unknown", "Matched", "NoFill"}
    /\ (nextState = "NoFill" => ~firstEverPartial)
    /\ (nextState = "PartialTerminal" => firstEverPartial)
    /\ firstState' = nextState
    /\ firstEverPartial' = (firstEverPartial \/ (nextState = "Partial"))
    /\ UNCHANGED <<staged, reservations, hedgeState, firstPermit, hedgePermit,
                    hedgeEverPartial, aborted, signing, submitted, childHalted,
                    halted>>

ExpireFirst ==
    /\ ~halted
    /\ firstState = "Authorized"
    /\ firstState' = "Expired"
    /\ UNCHANGED <<staged, reservations, hedgeState, firstPermit, hedgePermit,
                    firstEverPartial, hedgeEverPartial, aborted, signing,
                    submitted, childHalted, halted>>

AuthorizeHedge ==
    /\ ~halted
    /\ firstState = "Matched"
    /\ hedgeState = "None"
    /\ hedgePermit' = TRUE
    /\ hedgeState' = "Authorized"
    /\ UNCHANGED <<staged, reservations, firstState, firstPermit, aborted,
                    firstEverPartial, hedgeEverPartial, signing, submitted,
                    childHalted, halted>>

AdvanceHedge(nextState) ==
    /\ ~halted
    /\ hedgeState \in {"Authorized", "Active", "Partial", "Unknown"}
    /\ nextState \in {"Active", "Partial", "PartialTerminal", "Unknown", "Matched", "NoFill"}
    /\ (nextState = "NoFill" => ~hedgeEverPartial)
    /\ (nextState = "PartialTerminal" => hedgeEverPartial)
    /\ hedgeState' = nextState
    /\ hedgeEverPartial' = (hedgeEverPartial \/ (nextState = "Partial"))
    /\ UNCHANGED <<staged, reservations, firstState, firstPermit, hedgePermit,
                    firstEverPartial, aborted, signing, submitted, childHalted,
                    halted>>

AbortSafe ==
    /\ ~halted
    /\ staged
    /\ ~aborted
    /\ Safe(firstState)
    /\ Safe(hedgeState)
    /\ aborted' = TRUE
    /\ reservations' = 0
    /\ UNCHANGED <<staged, firstState, hedgeState, firstPermit, hedgePermit,
                    firstEverPartial, hedgeEverPartial, signing, submitted,
                    childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<staged, reservations, firstState, hedgeState, firstPermit,
                    hedgePermit, firstEverPartial, hedgeEverPartial, aborted,
                    signing, submitted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<staged, reservations, firstState, hedgeState, firstPermit,
                    hedgePermit, firstEverPartial, hedgeEverPartial, aborted,
                    signing, submitted, childHalted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ Stage
    \/ AuthorizeFirst
    \/ \E nextState \in LegStates: AdvanceFirst(nextState)
    \/ ExpireFirst
    \/ AuthorizeHedge
    \/ \E nextState \in LegStates: AdvanceHedge(nextState)
    \/ AbortSafe
    \/ ChildFailure
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ staged \in BOOLEAN
    /\ reservations \in {0, 2}
    /\ firstState \in LegStates
    /\ hedgeState \in LegStates
    /\ firstPermit \in BOOLEAN
    /\ hedgePermit \in BOOLEAN
    /\ firstEverPartial \in BOOLEAN
    /\ hedgeEverPartial \in BOOLEAN
    /\ aborted \in BOOLEAN
    /\ signing \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

HedgeRequiresFirstMatch == hedgePermit => firstState = "Matched"
PossibleExposureKeepsBoth ==
    (PossibleExposure(firstState) \/ PossibleExposure(hedgeState)) => reservations = 2
AbortHasNoPossibleExposure == aborted => Safe(firstState) /\ Safe(hedgeState)
AbortReleasesBoth == aborted => reservations = 0
NoLiveAuthority == ~signing /\ ~submitted
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
