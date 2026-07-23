------------------------- MODULE IntegrationSoak -------------------------
EXTENDS Integers, FiniteSets, TLC

Hours == 3
VARIABLES hour, btcCurrent, ethCurrent, finalBtc, finalEth, degraded, halted
vars == <<hour, btcCurrent, ethCurrent, finalBtc, finalEth, degraded, halted>>

Init ==
    /\ hour = 0 /\ btcCurrent = 0 /\ ethCurrent = 0
    /\ finalBtc = {} /\ finalEth = {}
    /\ degraded = FALSE /\ halted = FALSE

Tick ==
    /\ ~halted /\ hour < Hours
    /\ finalBtc' = finalBtc \cup {hour}
    /\ finalEth' = finalEth \cup {hour}
    /\ hour' = hour + 1
    /\ btcCurrent' = IF hour' < Hours THEN hour' ELSE -1
    /\ ethCurrent' = IF hour' < Hours THEN hour' ELSE -1
    /\ degraded' = FALSE
    /\ UNCHANGED halted

RecoverableFault ==
    /\ ~halted /\ hour < Hours
    /\ degraded' = ~degraded
    /\ UNCHANGED <<hour, btcCurrent, ethCurrent, finalBtc, finalEth, halted>>

IntegrityFault ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<hour, btcCurrent, ethCurrent, finalBtc, finalEth, degraded>>

Halted == halted /\ UNCHANGED vars
Completed == ~halted /\ hour = Hours /\ UNCHANGED vars

Next == Tick \/ RecoverableFault \/ IntegrityFault \/ Halted \/ Completed
Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ hour \in 0..Hours
    /\ btcCurrent \in {-1, 0, 1, 2}
    /\ ethCurrent \in {-1, 0, 1, 2}
    /\ finalBtc \subseteq 0..(Hours - 1)
    /\ finalEth \subseteq 0..(Hours - 1)
    /\ degraded \in BOOLEAN /\ halted \in BOOLEAN

SingleCurrentInvariant == btcCurrent = ethCurrent
CurrentWindowInvariant ==
    IF hour < Hours
    THEN /\ btcCurrent = hour /\ ethCurrent = hour
    ELSE /\ btcCurrent = -1 /\ ethCurrent = -1
FinalEvidenceInvariant ==
    /\ finalBtc = 0..(hour - 1)
    /\ finalEth = 0..(hour - 1)

=============================================================================
