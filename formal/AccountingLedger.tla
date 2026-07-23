-------------------------- MODULE AccountingLedger --------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS Capital, MaxAmount, ReservationIds, CommandIds

Statuses == {"none", "active", "released", "consumed"}
Contents == {"none", "A"}

VARIABLES available, reserved, inventoryCost, capitalCredit,
          tokenAvailable, tokenReserved, tokenExternal,
          reservationStatus, reservationAmount, seen, halted

vars == <<available, reserved, inventoryCost, capitalCredit,
          tokenAvailable, tokenReserved, tokenExternal,
          reservationStatus, reservationAmount, seen, halted>>

Init ==
    /\ available = Capital
    /\ reserved = 0
    /\ inventoryCost = 0
    /\ capitalCredit = -Capital
    /\ tokenAvailable = 0
    /\ tokenReserved = 0
    /\ tokenExternal = 0
    /\ reservationStatus = [id \in ReservationIds |-> "none"]
    /\ reservationAmount = [id \in ReservationIds |-> 0]
    /\ seen = [id \in CommandIds |-> "none"]
    /\ halted = FALSE

ReserveCash(command, reservation, amount) ==
    /\ ~halted /\ seen[command] = "none"
    /\ reservationStatus[reservation] = "none"
    /\ amount \in 1..MaxAmount /\ amount <= available
    /\ available' = available - amount
    /\ reserved' = reserved + amount
    /\ reservationStatus' = [reservationStatus EXCEPT ![reservation] = "active"]
    /\ reservationAmount' = [reservationAmount EXCEPT ![reservation] = amount]
    /\ seen' = [seen EXCEPT ![command] = "A"]
    /\ UNCHANGED <<inventoryCost, capitalCredit, tokenAvailable,
                    tokenReserved, tokenExternal, halted>>

ReleaseCash(command, reservation) ==
    /\ ~halted /\ seen[command] = "none"
    /\ reservationStatus[reservation] = "active"
    /\ available' = available + reservationAmount[reservation]
    /\ reserved' = reserved - reservationAmount[reservation]
    /\ reservationStatus' = [reservationStatus EXCEPT ![reservation] = "released"]
    /\ seen' = [seen EXCEPT ![command] = "A"]
    /\ UNCHANGED <<inventoryCost, capitalCredit, tokenAvailable,
                    tokenReserved, tokenExternal, reservationAmount, halted>>

ConfirmedBuy(command, reservation) ==
    /\ ~halted /\ seen[command] = "none"
    /\ reservationStatus[reservation] = "active"
    /\ reserved' = reserved - reservationAmount[reservation]
    /\ inventoryCost' = inventoryCost + reservationAmount[reservation]
    /\ tokenAvailable' = tokenAvailable + reservationAmount[reservation]
    /\ tokenExternal' = tokenExternal - reservationAmount[reservation]
    /\ reservationStatus' = [reservationStatus EXCEPT ![reservation] = "consumed"]
    /\ seen' = [seen EXCEPT ![command] = "A"]
    /\ UNCHANGED <<available, capitalCredit, tokenReserved,
                    reservationAmount, halted>>

ReserveToken(command, amount) ==
    /\ ~halted /\ seen[command] = "none"
    /\ amount \in 1..MaxAmount /\ amount <= tokenAvailable
    /\ tokenAvailable' = tokenAvailable - amount
    /\ tokenReserved' = tokenReserved + amount
    /\ seen' = [seen EXCEPT ![command] = "A"]
    /\ UNCHANGED <<available, reserved, inventoryCost, capitalCredit,
                    tokenExternal, reservationStatus, reservationAmount, halted>>

Duplicate(command) ==
    /\ ~halted /\ seen[command] = "A"
    /\ UNCHANGED vars

Conflict(command) ==
    /\ ~halted /\ seen[command] = "A"
    /\ halted' = TRUE
    /\ UNCHANGED <<available, reserved, inventoryCost, capitalCredit,
                    tokenAvailable, tokenReserved, tokenExternal,
                    reservationStatus, reservationAmount, seen>>

Terminal == halted /\ UNCHANGED vars

Next ==
    \/ \E command \in CommandIds, reservation \in ReservationIds,
          amount \in 1..MaxAmount: ReserveCash(command, reservation, amount)
    \/ \E command \in CommandIds, reservation \in ReservationIds:
          ReleaseCash(command, reservation)
    \/ \E command \in CommandIds, reservation \in ReservationIds:
          ConfirmedBuy(command, reservation)
    \/ \E command \in CommandIds, amount \in 1..MaxAmount:
          ReserveToken(command, amount)
    \/ \E command \in CommandIds: Duplicate(command)
    \/ \E command \in CommandIds: Conflict(command)
    \/ Terminal

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ available \in 0..Capital
    /\ reserved \in 0..Capital
    /\ inventoryCost \in 0..Capital
    /\ capitalCredit = -Capital
    /\ tokenAvailable \in 0..Capital
    /\ tokenReserved \in 0..Capital
    /\ tokenExternal \in -Capital..0
    /\ reservationStatus \in [ReservationIds -> Statuses]
    /\ reservationAmount \in [ReservationIds -> 0..MaxAmount]
    /\ seen \in [CommandIds -> Contents]
    /\ halted \in BOOLEAN

CollateralDoubleEntry ==
    available + reserved + inventoryCost + capitalCredit = 0

TokenDoubleEntry ==
    tokenAvailable + tokenReserved + tokenExternal = 0

RECURSIVE ReservedSum(_)

ReservedSum(ids) ==
    IF ids = {}
    THEN 0
    ELSE LET id == CHOOSE candidate \in ids: TRUE
         IN (IF reservationStatus[id] = "active" THEN reservationAmount[id] ELSE 0)
            + ReservedSum(ids \ {id})

ReservationBacked == reserved = ReservedSum(ReservationIds)

=============================================================================
