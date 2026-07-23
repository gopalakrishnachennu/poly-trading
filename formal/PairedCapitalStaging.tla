-------------------- MODULE PairedCapitalStaging --------------------
EXTENDS Naturals, TLC

Statuses == {"Idle", "FullyReserved", "Aborted"}

VARIABLES status, pairedEligible, reservationCount, authorityExists,
          abortObserved, childHalted, halted

vars == <<status, pairedEligible, reservationCount, authorityExists,
          abortObserved, childHalted, halted>>

Init ==
    /\ status = "Idle"
    /\ pairedEligible = FALSE
    /\ reservationCount = 0
    /\ authorityExists = FALSE
    /\ abortObserved = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

EvaluatePair(pass) ==
    /\ ~halted
    /\ status = "Idle"
    /\ pass \in BOOLEAN
    /\ pairedEligible' = pass
    /\ UNCHANGED <<status, reservationCount, authorityExists,
                    abortObserved, childHalted, halted>>

ReserveBoth ==
    /\ ~halted
    /\ status = "Idle"
    /\ pairedEligible
    /\ status' = "FullyReserved"
    /\ reservationCount' = 2
    /\ UNCHANGED <<pairedEligible, authorityExists, abortObserved,
                    childHalted, halted>>

AbortBoth ==
    /\ ~halted
    /\ status = "FullyReserved"
    /\ ~authorityExists
    /\ status' = "Aborted"
    /\ reservationCount' = 0
    /\ abortObserved' = TRUE
    /\ UNCHANGED <<pairedEligible, authorityExists, childHalted, halted>>

ReservationFailure ==
    /\ ~halted
    /\ status = "Idle"
    /\ pairedEligible
    /\ halted' = TRUE
    /\ UNCHANGED <<status, pairedEligible, reservationCount,
                    authorityExists, abortObserved, childHalted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<status, pairedEligible, reservationCount,
                    authorityExists, abortObserved>>

Terminal ==
    /\ status \in {"FullyReserved", "Aborted"}
    /\ UNCHANGED vars

Halted ==
    /\ halted
    /\ UNCHANGED vars

Next ==
    \/ \E pass \in BOOLEAN: EvaluatePair(pass)
    \/ ReserveBoth
    \/ AbortBoth
    \/ ReservationFailure
    \/ ChildFailure
    \/ Terminal
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ status \in Statuses
    /\ pairedEligible \in BOOLEAN
    /\ reservationCount \in 0..2
    /\ authorityExists \in BOOLEAN
    /\ abortObserved \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

NeverOneLeg == reservationCount # 1
FullStageHasBoth == (status = "FullyReserved") => reservationCount = 2
NoReservationOutsideFullStage == (status # "FullyReserved") => reservationCount = 0
StageRequiresEligibility == (status = "FullyReserved") => pairedEligible
AbortReleasesBoth == abortObserved => status = "Aborted" /\ reservationCount = 0
NoPlacementAuthority == ~authorityExists
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
