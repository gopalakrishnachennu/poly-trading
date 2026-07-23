------------------------- MODULE SessionRuntime -------------------------
EXTENDS Integers, TLC

MaxRecords == 3
Modes == {"Running", "Crashed", "IntegrityHalt"}
VARIABLES appended, durable, applied, mode
vars == <<appended, durable, applied, mode>>

Init ==
    /\ appended = 0 /\ durable = 0 /\ applied = 0
    /\ mode = "Running"
Append ==
    /\ mode = "Running" /\ appended < MaxRecords
    /\ appended' = appended + 1
    /\ UNCHANGED <<durable, applied, mode>>
Sync ==
    /\ mode = "Running" /\ durable < appended
    /\ durable' = appended
    /\ UNCHANGED <<appended, applied, mode>>
Apply ==
    /\ mode = "Running" /\ applied < durable
    /\ applied' = applied + 1
    /\ UNCHANGED <<appended, durable, mode>>
StateFailure ==
    /\ mode = "Running" /\ applied < durable
    /\ mode' = "IntegrityHalt"
    /\ UNCHANGED <<appended, durable, applied>>
Crash ==
    /\ mode = "Running" /\ mode' = "Crashed"
    /\ UNCHANGED <<appended, durable, applied>>
Recover ==
    /\ mode = "Crashed"
    /\ appended' = durable /\ applied' = durable /\ mode' = "Running"
    /\ UNCHANGED durable
Halted == mode = "IntegrityHalt" /\ UNCHANGED vars
Stutter == mode = "Running" /\ UNCHANGED vars

Next == Append \/ Sync \/ Apply \/ StateFailure \/ Crash \/ Recover \/ Halted \/ Stutter
Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ appended \in 0..MaxRecords /\ durable \in 0..MaxRecords
    /\ applied \in 0..MaxRecords /\ mode \in Modes
JournalBeforeApplyInvariant == applied <= durable
DurablePrefixInvariant == durable <= appended
HaltHasUnappliedDurableRecord == mode = "IntegrityHalt" => applied < durable
=============================================================================
