------------------ MODULE LiveDataPaperCertification ------------------
EXTENDS Integers, FiniteSets, TLC
CONSTANT MaxRecords
Folds == {"train", "validation", "test"}
Scenarios == {"chronology", "availability", "queue", "latency", "partial",
              "unknown", "cancel", "walk_forward", "frozen", "binding"}
VARIABLES registered, records, available, frozen, evaluated, covered, certified,
          realPnl, mutation, authority, halted
vars == <<registered, records, available, frozen, evaluated, covered, certified,
          realPnl, mutation, authority, halted>>
Init == /\ registered = FALSE /\ records = 0 /\ available = 0
        /\ frozen = FALSE /\ evaluated = {} /\ covered = {}
        /\ certified = FALSE /\ realPnl = FALSE /\ mutation = FALSE
        /\ authority = FALSE /\ halted = FALSE
Register == /\ ~registered /\ ~halted /\ registered' = TRUE
            /\ UNCHANGED <<records, available, frozen, evaluated, covered,
                           certified, realPnl, mutation, authority, halted>>
Ingest == /\ registered /\ ~frozen /\ ~certified /\ ~halted /\ records < MaxRecords
          /\ records' = records + 1 /\ available' = available + 1
          /\ covered' = covered \cup {"chronology", "availability"}
          /\ UNCHANGED <<registered, frozen, evaluated, certified, realPnl,
                         mutation, authority, halted>>
Freeze == /\ records = MaxRecords /\ ~frozen /\ ~certified /\ ~halted
          /\ frozen' = TRUE
          /\ UNCHANGED <<registered, records, available, evaluated, covered,
                         certified, realPnl, mutation, authority, halted>>
Evaluate(f) == /\ f \in Folds /\ registered /\ records = MaxRecords
               /\ ~certified /\ ~halted /\ (f # "test" \/ frozen)
               /\ evaluated' = evaluated \cup {f}
               /\ covered' = covered \cup {"queue", "latency", "partial",
                    "unknown", "cancel", "walk_forward", "binding"}
                    \cup (IF f = "test" THEN {"frozen"} ELSE {})
               /\ UNCHANGED <<registered, records, available, frozen, certified,
                              realPnl, mutation, authority, halted>>
Finalize == /\ frozen /\ evaluated = Folds /\ Scenarios \subseteq covered
            /\ ~certified /\ ~halted /\ certified' = TRUE
            /\ UNCHANGED <<registered, records, available, frozen, evaluated,
                           covered, realPnl, mutation, authority, halted>>
Fail == /\ ~certified /\ ~halted /\ halted' = TRUE
        /\ UNCHANGED <<registered, records, available, frozen, evaluated,
                       covered, certified, realPnl, mutation, authority>>
TerminalStutter == (certified \/ halted) /\ UNCHANGED vars
Next == Register \/ Ingest \/ Freeze \/ (\E f \in Folds : Evaluate(f))
        \/ Finalize \/ Fail \/ TerminalStutter
TypeInvariant == /\ registered \in BOOLEAN /\ records \in 0..MaxRecords
                 /\ available \in 0..MaxRecords /\ frozen \in BOOLEAN
                 /\ evaluated \subseteq Folds /\ covered \subseteq Scenarios
                 /\ certified \in BOOLEAN /\ realPnl \in BOOLEAN
                 /\ mutation \in BOOLEAN /\ authority \in BOOLEAN /\ halted \in BOOLEAN
AvailabilityBound == available = records
CompleteCertificate == certified => frozen /\ evaluated = Folds /\ Scenarios \subseteq covered
NoRealPnl == ~realPnl
NoMutation == ~mutation
NoAuthority == ~authority
HaltNotCertified == halted => ~certified
Spec == Init /\ [][Next]_vars
=======================================================================
