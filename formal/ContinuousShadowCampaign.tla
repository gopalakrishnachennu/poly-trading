-------------------- MODULE ContinuousShadowCampaign --------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS MaxTick, MinRollovers

Scenarios == {"steady", "budgets", "rollover", "restart", "venue_partition",
              "chain_partition", "deadman", "clock", "corruption"}
Disruptions == {"restart", "venue_partition", "chain_partition", "deadman"}

VARIABLES registered, tick, hour, rollovers, healthy, recovery, reason, covered,
          certified, realSoak, mutation, authority, halted
vars == <<registered, tick, hour, rollovers, healthy, recovery, reason, covered,
          certified, realSoak, mutation, authority, halted>>

Init == /\ registered = FALSE /\ tick = 0 /\ hour = 0 /\ rollovers = 0
        /\ healthy = FALSE /\ recovery = FALSE /\ reason = "none"
        /\ covered = {} /\ certified = FALSE /\ realSoak = FALSE
        /\ mutation = FALSE /\ authority = FALSE /\ halted = FALSE

Register == /\ ~registered /\ ~halted /\ registered' = TRUE
            /\ UNCHANGED <<tick, hour, rollovers, healthy, recovery, reason,
                           covered, certified, realSoak, mutation, authority, halted>>

Observe == /\ registered /\ ~recovery /\ ~certified /\ ~halted /\ tick < MaxTick
           /\ tick' = tick + 1 /\ healthy' = TRUE
           /\ hour' = IF tick' % 2 = 0 THEN hour + 1 ELSE hour
           /\ rollovers' = IF hour' > hour THEN rollovers + 1 ELSE rollovers
           /\ covered' = covered \cup {"steady", "budgets"}
                         \cup (IF hour' > hour THEN {"rollover"} ELSE {})
           /\ UNCHANGED <<registered, recovery, reason, certified, realSoak,
                          mutation, authority, halted>>

Disrupt(d) == /\ d \in Disruptions /\ registered /\ healthy /\ ~recovery
              /\ ~certified /\ ~halted /\ recovery' = TRUE /\ healthy' = FALSE
              /\ reason' = d
              /\ UNCHANGED <<registered, tick, hour, rollovers, covered,
                             certified, realSoak, mutation, authority, halted>>

Recover == /\ recovery /\ ~certified /\ ~halted /\ recovery' = FALSE
           /\ healthy' = TRUE /\ covered' = covered \cup {reason}
           /\ reason' = "none"
           /\ UNCHANGED <<registered, tick, hour, rollovers, certified,
                          realSoak, mutation, authority, halted>>

Fixture(s) == /\ s \in {"clock", "corruption"} /\ registered
              /\ ~certified /\ ~halted /\ covered' = covered \cup {s}
              /\ UNCHANGED <<registered, tick, hour, rollovers, healthy,
                             recovery, reason, certified, realSoak, mutation,
                             authority, halted>>

Finalize == /\ registered /\ healthy /\ ~recovery /\ rollovers >= MinRollovers
            /\ Scenarios \subseteq covered /\ ~certified /\ ~halted
            /\ certified' = TRUE
            /\ UNCHANGED <<registered, tick, hour, rollovers, healthy, recovery,
                           reason, covered, realSoak, mutation, authority, halted>>

Fail == /\ ~certified /\ ~halted /\ halted' = TRUE
        /\ UNCHANGED <<registered, tick, hour, rollovers, healthy, recovery,
                       reason, covered, certified, realSoak, mutation, authority>>
TerminalStutter == (certified \/ halted) /\ UNCHANGED vars

Next == Register \/ Observe \/ (\E d \in Disruptions : Disrupt(d)) \/ Recover
        \/ (\E s \in {"clock", "corruption"} : Fixture(s))
        \/ Finalize \/ Fail \/ TerminalStutter

TypeInvariant == /\ registered \in BOOLEAN /\ tick \in 0..MaxTick
                 /\ hour \in 0..MaxTick /\ rollovers \in 0..MaxTick
                 /\ healthy \in BOOLEAN /\ recovery \in BOOLEAN
                 /\ reason \in Disruptions \cup {"none"} /\ covered \subseteq Scenarios
                 /\ certified \in BOOLEAN /\ realSoak \in BOOLEAN
                 /\ mutation \in BOOLEAN /\ authority \in BOOLEAN /\ halted \in BOOLEAN
RecoveryInvalidates == recovery => ~healthy
CompleteCertificate == certified => healthy /\ ~recovery /\ Scenarios \subseteq covered
NoRealSoakClaim == ~realSoak
NoMutation == ~mutation
NoAuthority == ~authority
HaltNotCertified == halted => ~certified
Spec == Init /\ [][Next]_vars
=============================================================================
