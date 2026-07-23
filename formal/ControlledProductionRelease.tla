------------------- MODULE ControlledProductionRelease -------------------
EXTENDS Integers, FiniteSets, TLC

Scenarios == {"no_trade", "stages", "regions", "reconcile", "expiry",
              "incident", "dr", "rollback", "revoke", "governance"}
Regions == {"primary", "secondary"}

VARIABLES registered, approved, healthy, covered, rollback, revoked,
          certified, capital, order, authority, halted

vars == <<registered, approved, healthy, covered, rollback, revoked,
          certified, capital, order, authority, halted>>

Init == /\ registered = FALSE /\ approved = FALSE /\ healthy = {}
        /\ covered = {} /\ rollback = FALSE /\ revoked = FALSE
        /\ certified = FALSE /\ capital = FALSE /\ order = FALSE
        /\ authority = FALSE /\ halted = FALSE

Register == /\ ~registered /\ ~halted /\ registered' = TRUE
            /\ UNCHANGED <<approved, healthy, covered, rollback, revoked,
                           certified, capital, order, authority, halted>>

Approve == /\ registered /\ ~approved /\ ~halted /\ approved' = TRUE
           /\ UNCHANGED <<registered, healthy, covered, rollback, revoked,
                          certified, capital, order, authority, halted>>

Health(r) == /\ approved /\ r \in Regions /\ ~revoked /\ ~halted
             /\ healthy' = healthy \cup {r}
             /\ UNCHANGED <<registered, approved, covered, rollback, revoked,
                            certified, capital, order, authority, halted>>

Case(s) == /\ approved /\ s \in Scenarios /\ ~revoked /\ ~certified /\ ~halted
           /\ covered' = covered \cup {s}
           /\ rollback' = IF s \in {"incident", "dr", "rollback"}
                           THEN TRUE ELSE rollback
           /\ UNCHANGED <<registered, approved, healthy, revoked, certified,
                          capital, order, authority, halted>>

Revoke == /\ registered /\ ~revoked /\ ~certified /\ ~halted
          /\ revoked' = TRUE
          /\ UNCHANGED <<registered, approved, healthy, covered, rollback,
                         certified, capital, order, authority, halted>>

Finalize == /\ approved /\ Regions \subseteq healthy
            /\ Scenarios \subseteq covered /\ rollback /\ ~revoked
            /\ ~certified /\ ~halted /\ certified' = TRUE
            /\ UNCHANGED <<registered, approved, healthy, covered, rollback,
                           revoked, capital, order, authority, halted>>

Fail == /\ ~certified /\ ~halted /\ halted' = TRUE
        /\ UNCHANGED <<registered, approved, healthy, covered, rollback,
                       revoked, certified, capital, order, authority>>

TerminalStutter == (certified \/ revoked \/ halted) /\ UNCHANGED vars
Next == Register \/ Approve \/ (\E r \in Regions: Health(r))
        \/ (\E s \in Scenarios: Case(s)) \/ Revoke \/ Finalize \/ Fail
        \/ TerminalStutter

TypeInvariant == /\ registered \in BOOLEAN /\ approved \in BOOLEAN
                 /\ healthy \subseteq Regions /\ covered \subseteq Scenarios
                 /\ rollback \in BOOLEAN /\ revoked \in BOOLEAN
                 /\ certified \in BOOLEAN /\ capital \in BOOLEAN
                 /\ order \in BOOLEAN /\ authority \in BOOLEAN
                 /\ halted \in BOOLEAN
CompleteCertificate == certified => Regions \subseteq healthy
                                      /\ Scenarios \subseteq covered /\ rollback
RevocationBlocks == revoked => ~certified
NoCapital == ~capital
NoOrder == ~order
NoAuthority == ~authority
HaltNotCertified == halted => ~certified
Spec == Init /\ [][Next]_vars
=============================================================================
