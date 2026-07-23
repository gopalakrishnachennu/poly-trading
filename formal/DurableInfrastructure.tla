-------------------- MODULE DurableInfrastructure --------------------
EXTENDS Integers, TLC

Backends == {"postgres", "redpanda", "clickhouse", "parquet"}

VARIABLES registered, progress, schema, dropped, automaticRetry,
          authority, certified, halted

vars == <<registered, progress, schema, dropped, automaticRetry,
          authority, certified, halted>>

Init ==
  /\ registered = FALSE
  /\ progress = [b \in Backends |-> 0]
  /\ schema = [b \in Backends |-> 1]
  /\ dropped = FALSE
  /\ automaticRetry = FALSE
  /\ authority = FALSE
  /\ certified = FALSE
  /\ halted = FALSE

Register ==
  /\ ~registered /\ ~halted
  /\ registered' = TRUE
  /\ UNCHANGED <<progress, schema, dropped, automaticRetry,
                 authority, certified, halted>>

Advance(b) ==
  /\ b \in Backends
  /\ registered /\ ~certified /\ ~halted
  /\ progress[b] < 10
  /\ progress' = [progress EXCEPT ![b] = @ + 1]
  /\ schema' =
       IF progress[b] = 6 THEN [schema EXCEPT ![b] = 2]
       ELSE IF progress[b] = 7 THEN [schema EXCEPT ![b] = 1]
       ELSE schema
  /\ UNCHANGED <<registered, dropped, automaticRetry,
                 authority, certified, halted>>

Finalize ==
  /\ registered /\ ~certified /\ ~halted
  /\ \A b \in Backends : progress[b] = 10
  /\ certified' = TRUE
  /\ UNCHANGED <<registered, progress, schema, dropped,
                 automaticRetry, authority, halted>>

Fail ==
  /\ ~halted /\ ~certified
  /\ halted' = TRUE
  /\ UNCHANGED <<registered, progress, schema, dropped,
                 automaticRetry, authority, certified>>

TerminalStutter == (halted \/ certified) /\ UNCHANGED vars

Next ==
  \/ Register
  \/ \E b \in Backends : Advance(b)
  \/ Finalize
  \/ Fail
  \/ TerminalStutter

TypeInvariant ==
  /\ registered \in BOOLEAN
  /\ progress \in [Backends -> 0..10]
  /\ schema \in [Backends -> {1, 2}]
  /\ dropped \in BOOLEAN
  /\ automaticRetry \in BOOLEAN
  /\ authority \in BOOLEAN
  /\ certified \in BOOLEAN
  /\ halted \in BOOLEAN

MigrationBound == \A b \in Backends :
  (progress[b] = 7 => schema[b] = 2) /\ (progress[b] >= 8 => schema[b] = 1)
CompleteBeforeCertified == certified => \A b \in Backends : progress[b] = 10
NoDrop == ~dropped
NoAutomaticRetry == ~automaticRetry
NoAuthority == ~authority
HaltNotCertified == halted => ~certified

Spec == Init /\ [][Next]_vars

======================================================================
