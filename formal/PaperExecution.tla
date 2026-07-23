-------------------------- MODULE PaperExecution ----------------------------
EXTENDS Naturals, TLC

ActiveStates == {"Submitted", "Delayed", "Acknowledged", "Live",
                 "PartiallyMatched", "CancelPending", "Unknown"}
TerminalStates == {"FullyMatched", "Canceled", "Rejected"}

VARIABLES state, fills, handoffs, halted

vars == <<state, fills, handoffs, halted>>

Init ==
    /\ state = "Absent"
    /\ fills = 0
    /\ handoffs = 0
    /\ halted = FALSE

Submit ==
    /\ ~halted
    /\ state = "Absent"
    /\ state' = "Submitted"
    /\ UNCHANGED <<fills, handoffs, halted>>

Move(nextState) ==
    /\ ~halted
    /\ state \in ActiveStates
    /\ nextState \in ActiveStates
    /\ state' = nextState
    /\ UNCHANGED <<fills, handoffs, halted>>

PartialFill ==
    /\ ~halted
    /\ state \in ActiveStates
    /\ fills < 1
    /\ fills' = fills + 1
    /\ handoffs' = handoffs + 1
    /\ state' = IF state = "CancelPending" THEN "CancelPending"
                 ELSE "PartiallyMatched"
    /\ UNCHANGED halted

FullFill ==
    /\ ~halted
    /\ state \in ActiveStates
    /\ fills < 2
    /\ fills' = fills + 1
    /\ handoffs' = handoffs + 1
    /\ state' = "FullyMatched"
    /\ UNCHANGED halted

CancelAccepted ==
    /\ ~halted
    /\ state = "CancelPending"
    /\ state' = "Canceled"
    /\ UNCHANGED <<fills, handoffs, halted>>

Reject ==
    /\ ~halted
    /\ state \in ActiveStates
    /\ state' = "Rejected"
    /\ UNCHANGED <<fills, handoffs, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<state, fills, handoffs>>

Halted ==
    /\ halted
    /\ UNCHANGED vars

Terminal ==
    /\ state \in TerminalStates
    /\ UNCHANGED vars

Next ==
    \/ Submit
    \/ \E nextState \in ActiveStates: Move(nextState)
    \/ PartialFill
    \/ FullFill
    \/ CancelAccepted
    \/ Reject
    \/ IntegrityFailure
    \/ Halted
    \/ Terminal

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ state \in {"Absent"} \cup ActiveStates \cup TerminalStates
    /\ fills \in 0..2
    /\ handoffs \in 0..2
    /\ halted \in BOOLEAN

EveryFillHasOneHandoff == fills = handoffs
UnknownIsNonTerminal == state = "Unknown" => state \notin TerminalStates
CancelPendingCanStillCarryFill ==
    state = "CancelPending" => fills \in 0..1
TerminalStatesAreExclusive ==
    state \in TerminalStates => state \notin ActiveStates
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
