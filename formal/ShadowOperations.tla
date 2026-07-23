------------------------ MODULE ShadowOperations ------------------------
EXTENDS TLC

Modes == {"Starting", "Ready", "Degraded", "Draining", "Stopped", "Halted"}
VARIABLES mode, drainRequested, haltLatched
vars == <<mode, drainRequested, haltLatched>>

Init ==
    /\ mode = "Starting"
    /\ drainRequested = FALSE
    /\ haltLatched = FALSE

Healthy ==
    /\ mode \in {"Starting", "Ready", "Degraded"}
    /\ ~drainRequested /\ ~haltLatched
    /\ mode' = "Ready"
    /\ UNCHANGED <<drainRequested, haltLatched>>

BudgetExceeded ==
    /\ mode \in {"Starting", "Ready", "Degraded"}
    /\ ~drainRequested /\ ~haltLatched
    /\ mode' = "Degraded"
    /\ UNCHANGED <<drainRequested, haltLatched>>

BeginDrain ==
    /\ mode \in {"Starting", "Ready", "Degraded", "Draining"}
    /\ ~haltLatched
    /\ mode' = "Draining" /\ drainRequested' = TRUE
    /\ UNCHANGED haltLatched

Stop ==
    /\ mode = "Draining" /\ drainRequested /\ ~haltLatched
    /\ mode' = "Stopped"
    /\ UNCHANGED <<drainRequested, haltLatched>>

IntegrityFailure ==
    /\ mode \notin {"Stopped", "Halted"}
    /\ mode' = "Halted" /\ haltLatched' = TRUE
    /\ UNCHANGED drainRequested

Terminal == mode \in {"Stopped", "Halted"} /\ UNCHANGED vars
Next == Healthy \/ BudgetExceeded \/ BeginDrain \/ Stop \/ IntegrityFailure \/ Terminal
Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ mode \in Modes
    /\ drainRequested \in BOOLEAN
    /\ haltLatched \in BOOLEAN
DrainOrderingInvariant == mode = "Stopped" => drainRequested
NoReadyAfterDrainInvariant == drainRequested => mode \notin {"Ready", "Degraded"}
HaltLatchInvariant == haltLatched <=> mode = "Halted"
=============================================================================
