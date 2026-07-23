-------------------------- MODULE CapitalReservation --------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS Capital, MaxOrder, OrderIds

VARIABLES cash, reserved, status, amount

vars == <<cash, reserved, status, amount>>

Statuses == {"none", "reserved", "consumed", "released"}

OrderSymmetry == Permutations(OrderIds)

Init ==
    /\ cash = Capital
    /\ reserved = 0
    /\ status = [order \in OrderIds |-> "none"]
    /\ amount = [order \in OrderIds |-> 0]

Reserve(order, value) ==
    /\ order \in OrderIds
    /\ status[order] = "none"
    /\ value \in 1..MaxOrder
    /\ reserved + value <= cash
    /\ reserved' = reserved + value
    /\ status' = [status EXCEPT ![order] = "reserved"]
    /\ amount' = [amount EXCEPT ![order] = value]
    /\ UNCHANGED cash

Release(order) ==
    /\ order \in OrderIds
    /\ status[order] = "reserved"
    /\ reserved' = reserved - amount[order]
    /\ status' = [status EXCEPT ![order] = "released"]
    /\ UNCHANGED <<cash, amount>>

Consume(order) ==
    /\ order \in OrderIds
    /\ status[order] = "reserved"
    /\ cash' = cash - amount[order]
    /\ reserved' = reserved - amount[order]
    /\ status' = [status EXCEPT ![order] = "consumed"]
    /\ UNCHANGED amount

Next ==
    \/ \E order \in OrderIds, value \in 1..MaxOrder: Reserve(order, value)
    \/ \E order \in OrderIds: Release(order)
    \/ \E order \in OrderIds: Consume(order)
    \/ UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ cash \in 0..Capital
    /\ reserved \in 0..Capital
    /\ status \in [OrderIds -> Statuses]
    /\ amount \in [OrderIds -> 0..MaxOrder]

ReservationBacked == reserved <= cash

RECURSIVE ReservedSum(_)

ReservedSum(orders) ==
    IF orders = {}
    THEN 0
    ELSE LET order == CHOOSE candidate \in orders: TRUE
         IN (IF status[order] = "reserved" THEN amount[order] ELSE 0)
            + ReservedSum(orders \ {order})

ReservationAccounting == reserved = ReservedSum(OrderIds)

NoInactiveAmount ==
    \A order \in OrderIds:
        status[order] = "none" => amount[order] = 0

Safety == TypeOK /\ ReservationBacked /\ ReservationAccounting /\ NoInactiveAmount

=============================================================================
