--------------------- MODULE SettlementReconciliation ---------------------
EXTENDS TLC

CONSTANT Trades

Statuses == {"registered", "matched", "mined", "retrying", "confirmed", "failed"}
Modes == {"Awaiting", "Pending", "Ready", "Halted"}

VARIABLES status, ledgerPosted, sourcesObserved, assetsEqual, mode, halted
vars == <<status, ledgerPosted, sourcesObserved, assetsEqual, mode, halted>>

Init ==
    /\ status = [trade \in Trades |-> "registered"]
    /\ ledgerPosted = [trade \in Trades |-> FALSE]
    /\ sourcesObserved = FALSE
    /\ assetsEqual = FALSE
    /\ mode = "Awaiting"
    /\ halted = FALSE

Match(trade) ==
    /\ ~halted /\ status[trade] = "registered"
    /\ status' = [status EXCEPT ![trade] = "matched"]
    /\ mode' = "Pending"
    /\ UNCHANGED <<ledgerPosted, sourcesObserved, assetsEqual, halted>>

Mine(trade) ==
    /\ ~halted /\ status[trade] \in {"matched", "retrying"}
    /\ status' = [status EXCEPT ![trade] = "mined"]
    /\ mode' = "Pending"
    /\ UNCHANGED <<ledgerPosted, sourcesObserved, assetsEqual, halted>>

Retry(trade) ==
    /\ ~halted /\ status[trade] \in {"matched", "mined"}
    /\ status' = [status EXCEPT ![trade] = "retrying"]
    /\ mode' = "Pending"
    /\ UNCHANGED <<ledgerPosted, sourcesObserved, assetsEqual, halted>>

Confirm(trade) ==
    /\ ~halted /\ status[trade] = "mined"
    /\ status' = [status EXCEPT ![trade] = "confirmed"]
    /\ mode' = "Pending"
    /\ UNCHANGED <<ledgerPosted, sourcesObserved, assetsEqual, halted>>

Fail(trade) ==
    /\ ~halted /\ status[trade] = "retrying"
    /\ status' = [status EXCEPT ![trade] = "failed"]
    /\ mode' = "Pending"
    /\ UNCHANGED <<ledgerPosted, sourcesObserved, assetsEqual, halted>>

PostConfirmed(trade) ==
    /\ ~halted /\ status[trade] = "confirmed" /\ ~ledgerPosted[trade]
    /\ ledgerPosted' = [ledgerPosted EXCEPT ![trade] = TRUE]
    /\ mode' = "Pending"
    /\ UNCHANGED <<status, sourcesObserved, assetsEqual, halted>>

AllTerminalAndConsistent ==
    \A trade \in Trades:
        \/ status[trade] = "failed" /\ ~ledgerPosted[trade]
        \/ status[trade] = "confirmed" /\ ledgerPosted[trade]

ReconcileEqual ==
    /\ ~halted
    /\ sourcesObserved' = TRUE /\ assetsEqual' = TRUE
    /\ mode' = IF AllTerminalAndConsistent THEN "Ready" ELSE "Pending"
    /\ UNCHANGED <<status, ledgerPosted, halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE /\ mode' = "Halted"
    /\ UNCHANGED <<status, ledgerPosted, sourcesObserved, assetsEqual>>

Terminal == halted /\ UNCHANGED vars

Next ==
    \/ \E trade \in Trades: Match(trade)
    \/ \E trade \in Trades: Mine(trade)
    \/ \E trade \in Trades: Retry(trade)
    \/ \E trade \in Trades: Confirm(trade)
    \/ \E trade \in Trades: Fail(trade)
    \/ \E trade \in Trades: PostConfirmed(trade)
    \/ ReconcileEqual
    \/ IntegrityFailure
    \/ Terminal

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ status \in [Trades -> Statuses]
    /\ ledgerPosted \in [Trades -> BOOLEAN]
    /\ sourcesObserved \in BOOLEAN
    /\ assetsEqual \in BOOLEAN
    /\ mode \in Modes
    /\ halted \in BOOLEAN

NonConfirmedNeverPosted ==
    \A trade \in Trades: ledgerPosted[trade] => status[trade] = "confirmed"

ReadyRequiresExactTerminalTruth ==
    mode = "Ready" =>
        /\ sourcesObserved /\ assetsEqual /\ ~halted
        /\ AllTerminalAndConsistent

HaltLatch == halted <=> mode = "Halted"

=============================================================================
