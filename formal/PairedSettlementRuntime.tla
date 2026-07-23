-------------------- MODULE PairedSettlementRuntime --------------------
EXTENDS Naturals, TLC

VARIABLES handoffs, registered, confirmed, failed, posted,
          executionTerminal, reconciled, reservations, locked,
          finalized, signing, submitted, childHalted, halted

vars == <<handoffs, registered, confirmed, failed, posted,
          executionTerminal, reconciled, reservations, locked,
          finalized, signing, submitted, childHalted, halted>>

Init ==
    /\ handoffs = 0
    /\ registered = 0
    /\ confirmed = 0
    /\ failed = 0
    /\ posted = 0
    /\ executionTerminal = FALSE
    /\ reconciled = FALSE
    /\ reservations = 2
    /\ locked = FALSE
    /\ finalized = FALSE
    /\ signing = FALSE
    /\ submitted = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

CreateHandoff ==
    /\ ~halted
    /\ handoffs < 2
    /\ handoffs' = handoffs + 1
    /\ reconciled' = FALSE
    /\ UNCHANGED <<registered, confirmed, failed, posted,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

RegisterAuthentic ==
    /\ ~halted
    /\ registered < handoffs
    /\ registered' = registered + 1
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, confirmed, failed, posted,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

ConfirmTrade ==
    /\ ~halted
    /\ confirmed + failed < registered
    /\ confirmed' = confirmed + 1
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, registered, failed, posted,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

FailTrade ==
    /\ ~halted
    /\ confirmed + failed < registered
    /\ failed' = failed + 1
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, registered, confirmed, posted,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

PostConfirmed ==
    /\ ~halted
    /\ posted < confirmed
    /\ posted' = posted + 1
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

TerminalExecution ==
    /\ ~halted
    /\ executionTerminal' = TRUE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    reconciled, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

ReconcileCurrent ==
    /\ ~halted
    /\ registered = handoffs
    /\ confirmed + failed = registered
    /\ posted = confirmed
    /\ reconciled' = TRUE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    executionTerminal, reservations, locked, finalized,
                    signing, submitted, childHalted, halted>>

LockPair ==
    /\ ~halted
    /\ reconciled
    /\ posted = 2
    /\ ~locked
    /\ locked' = TRUE
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    executionTerminal, reservations, finalized,
                    signing, submitted, childHalted, halted>>

FinalizePair ==
    /\ ~halted
    /\ executionTerminal
    /\ reconciled
    /\ reservations = 2
    /\ reservations' = 0
    /\ finalized' = TRUE
    /\ reconciled' = FALSE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    executionTerminal, locked, signing, submitted,
                    childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    executionTerminal, reconciled, reservations, locked,
                    finalized, signing, submitted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<handoffs, registered, confirmed, failed, posted,
                    executionTerminal, reconciled, reservations, locked,
                    finalized, signing, submitted, childHalted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ CreateHandoff
    \/ RegisterAuthentic
    \/ ConfirmTrade
    \/ FailTrade
    \/ PostConfirmed
    \/ TerminalExecution
    \/ ReconcileCurrent
    \/ LockPair
    \/ FinalizePair
    \/ ChildFailure
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ handoffs \in 0..2
    /\ registered \in 0..2
    /\ confirmed \in 0..2
    /\ failed \in 0..2
    /\ posted \in 0..2
    /\ executionTerminal \in BOOLEAN
    /\ reconciled \in BOOLEAN
    /\ reservations \in {0, 2}
    /\ locked \in BOOLEAN
    /\ finalized \in BOOLEAN
    /\ signing \in BOOLEAN
    /\ submitted \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

RegistrationHasOrigin == registered <= handoffs
ConfirmedIsRegistered == confirmed + failed <= registered
ConfirmedOnlyPosting == posted <= confirmed
FailedNeverPosted == posted + failed <= registered
ReconciledTruth ==
    reconciled => registered = handoffs /\ confirmed + failed = registered /\ posted = confirmed
FinalizationRequiresTruth == finalized => executionTerminal /\ reservations = 0
RetentionBeforeFinalization == ~finalized => reservations = 2
PairedReleaseAtomic == reservations \in {0, 2}
NoLiveAuthority == ~signing /\ ~submitted
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
