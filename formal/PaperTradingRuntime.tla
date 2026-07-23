---------------------- MODULE PaperTradingRuntime ----------------------
EXTENDS FiniteSets, Naturals, TLC

Stages == {"Ready", "RiskApproved", "Reserved", "PolicyPermitted",
           "Executing", "HandoffRegistered"}
Faults == {"BeforeRisk", "BeforeExecution", "BeforeHandoff"}

VARIABLES stage, armed, injected, used, handoffs, childHalted, halted

vars == <<stage, armed, injected, used, handoffs, childHalted, halted>>

Init ==
    /\ stage = "Ready"
    /\ armed = "None"
    /\ injected = {}
    /\ used = {}
    /\ handoffs = 0
    /\ childHalted = FALSE
    /\ halted = FALSE

ArmFault(point) ==
    /\ ~halted
    /\ point \in Faults
    /\ point \notin injected
    /\ armed = "None"
    /\ armed' = point
    /\ injected' = injected \cup {point}
    /\ UNCHANGED <<stage, used, handoffs, childHalted, halted>>

RiskFault ==
    /\ ~halted
    /\ stage = "Ready"
    /\ armed = "BeforeRisk"
    /\ armed' = "None"
    /\ used' = used \cup {"BeforeRisk"}
    /\ UNCHANGED <<stage, injected, handoffs, childHalted, halted>>

RiskPass ==
    /\ ~halted
    /\ stage = "Ready"
    /\ armed # "BeforeRisk"
    /\ stage' = "RiskApproved"
    /\ UNCHANGED <<armed, injected, used, handoffs, childHalted, halted>>

Reserve ==
    /\ ~halted
    /\ stage = "RiskApproved"
    /\ stage' = "Reserved"
    /\ UNCHANGED <<armed, injected, used, handoffs, childHalted, halted>>

PermitPolicy ==
    /\ ~halted
    /\ stage = "Reserved"
    /\ stage' = "PolicyPermitted"
    /\ UNCHANGED <<armed, injected, used, handoffs, childHalted, halted>>

ExecutionFault ==
    /\ ~halted
    /\ stage = "PolicyPermitted"
    /\ armed = "BeforeExecution"
    /\ armed' = "None"
    /\ used' = used \cup {"BeforeExecution"}
    /\ UNCHANGED <<stage, injected, handoffs, childHalted, halted>>

Execute ==
    /\ ~halted
    /\ stage = "PolicyPermitted"
    /\ armed # "BeforeExecution"
    /\ stage' = "Executing"
    /\ UNCHANGED <<armed, injected, used, handoffs, childHalted, halted>>

HandoffFault ==
    /\ ~halted
    /\ stage = "Executing"
    /\ armed = "BeforeHandoff"
    /\ armed' = "None"
    /\ used' = used \cup {"BeforeHandoff"}
    /\ UNCHANGED <<stage, injected, handoffs, childHalted, halted>>

RegisterHandoff ==
    /\ ~halted
    /\ stage = "Executing"
    /\ armed # "BeforeHandoff"
    /\ handoffs = 0
    /\ stage' = "HandoffRegistered"
    /\ handoffs' = 1
    /\ UNCHANGED <<armed, injected, used, childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<stage, armed, injected, used, handoffs>>

IntegrityFault ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<stage, armed, injected, used, handoffs, childHalted>>

Halted ==
    /\ halted
    /\ UNCHANGED vars

Completed ==
    /\ stage = "HandoffRegistered"
    /\ UNCHANGED vars

Next ==
    \/ \E point \in Faults: ArmFault(point)
    \/ RiskFault
    \/ RiskPass
    \/ Reserve
    \/ PermitPolicy
    \/ ExecutionFault
    \/ Execute
    \/ HandoffFault
    \/ RegisterHandoff
    \/ ChildFailure
    \/ IntegrityFault
    \/ Halted
    \/ Completed

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ stage \in Stages
    /\ armed \in Faults \cup {"None"}
    /\ injected \subseteq Faults
    /\ used \subseteq Faults
    /\ handoffs \in 0..1
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

OrderedPipeline ==
    /\ stage \in {"Reserved", "PolicyPermitted", "Executing", "HandoffRegistered"}
       => stage # "RiskApproved"
    /\ stage \in {"PolicyPermitted", "Executing", "HandoffRegistered"}
       => stage # "Reserved"
    /\ stage \in {"Executing", "HandoffRegistered"}
       => stage # "PolicyPermitted"

HandoffIsUnique == handoffs <= 1
HandoffRequiresExecution == handoffs = 1 => stage = "HandoffRegistered"
FaultUseIsOneShot ==
    /\ used \subseteq injected
    /\ armed # "None" => armed \notin used
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
