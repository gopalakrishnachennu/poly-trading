-------------------------- MODULE MarketSession --------------------------
EXTENDS Integers, TLC

Phases == {"Upcoming", "Degraded", "Ready", "Awaiting", "Finalized"}
VARIABLES clock, phase1, phase2, final1, final2, seenFinal1, seenFinal2, halted
vars == <<clock, phase1, phase2, final1, final2, seenFinal1, seenFinal2, halted>>

Init ==
    /\ clock = 0 /\ phase1 = "Degraded" /\ phase2 = "Upcoming"
    /\ final1 = FALSE /\ final2 = FALSE
    /\ seenFinal1 = FALSE /\ seenFinal2 = FALSE /\ halted = FALSE

Ready1 ==
    /\ ~halted /\ clock = 0 /\ phase1' = "Ready"
    /\ UNCHANGED <<clock, phase2, final1, final2, seenFinal1, seenFinal2, halted>>
Ready2 ==
    /\ ~halted /\ clock = 1 /\ phase2' = "Ready"
    /\ UNCHANGED <<clock, phase1, final1, final2, seenFinal1, seenFinal2, halted>>
Tick ==
    /\ ~halted /\ clock < 2 /\ clock' = clock + 1
    /\ phase1' = IF clock' = 1 THEN "Awaiting" ELSE phase1
    /\ phase2' = IF clock' = 1 THEN "Degraded"
                  ELSE IF clock' = 2 THEN "Awaiting" ELSE phase2
    /\ UNCHANGED <<final1, final2, seenFinal1, seenFinal2, halted>>
Finalize1 ==
    /\ ~halted /\ clock >= 1
    /\ final1' = TRUE /\ seenFinal1' = TRUE /\ phase1' = "Finalized"
    /\ UNCHANGED <<clock, phase2, final2, seenFinal2, halted>>
Finalize2 ==
    /\ ~halted /\ clock >= 2
    /\ final2' = TRUE /\ seenFinal2' = TRUE /\ phase2' = "Finalized"
    /\ UNCHANGED <<clock, phase1, final1, seenFinal1, halted>>
Halt ==
    /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<clock, phase1, phase2, final1, final2, seenFinal1, seenFinal2>>
Stopped == halted /\ UNCHANGED vars

Next == Ready1 \/ Ready2 \/ Tick \/ Finalize1 \/ Finalize2 \/ Halt \/ Stopped
Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ clock \in 0..2 /\ phase1 \in Phases /\ phase2 \in Phases
    /\ final1 \in BOOLEAN /\ final2 \in BOOLEAN
    /\ seenFinal1 \in BOOLEAN /\ seenFinal2 \in BOOLEAN /\ halted \in BOOLEAN
ReadinessWindowInvariant ==
    /\ (phase1 = "Ready" => clock = 0)
    /\ (phase2 = "Ready" => clock = 1)
SingleCurrentInvariant == ~(phase1 = "Ready" /\ phase2 = "Ready")
FinalEvidenceInvariant ==
    /\ (phase1 = "Finalized" => final1)
    /\ (phase2 = "Finalized" => final2)
FinalImmutabilityInvariant == /\ (seenFinal1 => final1) /\ (seenFinal2 => final2)
=============================================================================
