------------------------- MODULE FeedSupervisor -------------------------
EXTENDS Naturals

CONSTANTS MaxTime, MaxEpoch, MaxSeq

VARIABLES mode,
          now,
          marketEpoch,
          referenceEpoch,
          marketSeq,
          referenceSeq,
          everHalted

vars == << mode, now, marketEpoch, referenceEpoch,
           marketSeq, referenceSeq, everHalted >>

Modes == {"STARTING", "READY", "DEGRADED", "HALTED"}

Init ==
    /\ mode = "STARTING"
    /\ now = 0
    /\ marketEpoch = 0
    /\ referenceEpoch = 0
    /\ marketSeq = 0
    /\ referenceSeq = 0
    /\ everHalted = FALSE

InputBounds(n, me, re, ms, rs) ==
    /\ n \in 0..MaxTime
    /\ me \in 0..MaxEpoch
    /\ re \in 0..MaxEpoch
    /\ ms \in 0..MaxSeq
    /\ rs \in 0..MaxSeq

AcceptObservation ==
    \E n \in 0..MaxTime,
       me \in 0..MaxEpoch,
       re \in 0..MaxEpoch,
       ms \in 0..MaxSeq,
       rs \in 0..MaxSeq,
       healthy \in BOOLEAN:
        /\ ~everHalted
        /\ InputBounds(n, me, re, ms, rs)
        /\ n >= now
        /\ me >= marketEpoch
        /\ re >= referenceEpoch
        /\ ms >= marketSeq
        /\ rs >= referenceSeq
        /\ mode' = IF healthy THEN "READY" ELSE "DEGRADED"
        /\ now' = n
        /\ marketEpoch' = me
        /\ referenceEpoch' = re
        /\ marketSeq' = ms
        /\ referenceSeq' = rs
        /\ everHalted' = FALSE

IntegrityFailure ==
    \E n \in 0..MaxTime,
       me \in 0..MaxEpoch,
       re \in 0..MaxEpoch,
       ms \in 0..MaxSeq,
       rs \in 0..MaxSeq:
        /\ ~everHalted
        /\ InputBounds(n, me, re, ms, rs)
        /\ \/ n < now
           \/ me < marketEpoch
           \/ re < referenceEpoch
           \/ ms < marketSeq
           \/ rs < referenceSeq
        /\ mode' = "HALTED"
        /\ everHalted' = TRUE
        /\ UNCHANGED << now, marketEpoch, referenceEpoch,
                        marketSeq, referenceSeq >>

HaltedStep ==
    /\ everHalted
    /\ mode = "HALTED"
    /\ UNCHANGED vars

Next == AcceptObservation \/ IntegrityFailure \/ HaltedStep

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ mode \in Modes
    /\ now \in 0..MaxTime
    /\ marketEpoch \in 0..MaxEpoch
    /\ referenceEpoch \in 0..MaxEpoch
    /\ marketSeq \in 0..MaxSeq
    /\ referenceSeq \in 0..MaxSeq
    /\ everHalted \in BOOLEAN

HaltIsAbsorbing == everHalted => mode = "HALTED"
ReadyIsNotHalted == mode = "READY" => ~everHalted

=============================================================================
