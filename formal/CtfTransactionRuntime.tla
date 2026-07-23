-------------------- MODULE CtfTransactionRuntime --------------------
EXTENDS Naturals, TLC

States == {"None", "Requested", "Pending", "Retrying", "Confirmed", "Failed"}

VARIABLES state, merge, backing, externalBound, posted, released,
          duplicateCount, signing, submitted, childHalted, halted

vars == <<state, merge, backing, externalBound, posted, released,
          duplicateCount, signing, submitted, childHalted, halted>>

Init ==
    /\ state = "None"
    /\ merge = FALSE
    /\ backing = FALSE
    /\ externalBound = FALSE
    /\ posted = FALSE
    /\ released = FALSE
    /\ duplicateCount = 0
    /\ signing = FALSE
    /\ submitted = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

Request(kindIsMerge) ==
    /\ ~halted
    /\ state = "None"
    /\ state' = "Requested"
    /\ merge' = kindIsMerge
    /\ backing' = TRUE
    /\ UNCHANGED <<externalBound, posted, released, duplicateCount,
                    signing, submitted, childHalted, halted>>

Pending ==
    /\ ~halted
    /\ state = "Requested"
    /\ state' = "Pending"
    /\ externalBound' = TRUE
    /\ UNCHANGED <<merge, backing, posted, released, duplicateCount,
                    signing, submitted, childHalted, halted>>

Retry ==
    /\ ~halted
    /\ externalBound
    /\ state \in {"Pending", "Retrying"}
    /\ state' = "Retrying"
    /\ UNCHANGED <<merge, backing, externalBound, posted, released,
                    duplicateCount, signing, submitted, childHalted, halted>>

Confirm ==
    /\ ~halted
    /\ externalBound
    /\ state \in {"Pending", "Retrying"}
    /\ state' = "Confirmed"
    /\ posted' = TRUE
    /\ backing' = FALSE
    /\ UNCHANGED <<merge, externalBound, released, duplicateCount,
                    signing, submitted, childHalted, halted>>

Fail ==
    /\ ~halted
    /\ state \in {"Requested", "Pending", "Retrying"}
    /\ state' = "Failed"
    /\ backing' = merge
    /\ released' = ~merge
    /\ UNCHANGED <<merge, externalBound, posted, duplicateCount,
                    signing, submitted, childHalted, halted>>

DuplicateSubmission ==
    /\ ~halted
    /\ duplicateCount < 2
    /\ externalBound
    /\ state \in {"Pending", "Retrying"}
    /\ duplicateCount' = duplicateCount + 1
    /\ UNCHANGED <<state, merge, backing, externalBound, posted, released,
                    signing, submitted, childHalted, halted>>

DuplicateTerminal ==
    /\ ~halted
    /\ duplicateCount < 2
    /\ state \in {"Confirmed", "Failed"}
    /\ duplicateCount' = duplicateCount + 1
    /\ UNCHANGED <<state, merge, backing, externalBound, posted, released,
                    signing, submitted, childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<state, merge, backing, externalBound, posted, released,
                    duplicateCount, signing, submitted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<state, merge, backing, externalBound, posted, released,
                    duplicateCount, signing, submitted, childHalted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ \E kind \in BOOLEAN: Request(kind)
    \/ Pending
    \/ Retry
    \/ Confirm
    \/ Fail
    \/ DuplicateSubmission
    \/ DuplicateTerminal
    \/ ChildFailure
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ state \in States
    /\ merge \in BOOLEAN
    /\ backing \in BOOLEAN
    /\ externalBound \in BOOLEAN
    /\ posted \in BOOLEAN
    /\ released \in BOOLEAN
    /\ duplicateCount \in 0..2
    /\ signing \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

BackingBeforePending == state \in {"Requested", "Pending", "Retrying"} => backing
ConfirmedOnlyPosting == posted => state = "Confirmed"
ConfirmedPostsOnce == state = "Confirmed" => posted /\ ~backing
FailedPolicy == state = "Failed" => (~merge => released /\ ~backing) /\ (merge => backing)
TerminalImmutable == state \in {"Confirmed", "Failed"} => posted = (state = "Confirmed")
NoLiveAuthority == ~signing /\ ~submitted
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
