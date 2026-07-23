------------------------ MODULE OrderIntentPolicy ---------------------------
EXTENDS Naturals, TLC

Modes == {"Unknown", "Normal", "Restarting", "PostOnly", "CancelOnly",
          "TradingDisabled", "Recovering"}
CancelModes == {"Normal", "PostOnly", "CancelOnly", "TradingDisabled", "Recovering"}

VARIABLES mode, riskApproved, policyFresh, exactOrder, postOnlySafe,
          approvalUsed, placementPermits, orderState, cancelWindowOpen,
          lastPlacePermit, lastCancelPermit, halted

vars == <<mode, riskApproved, policyFresh, exactOrder, postOnlySafe,
          approvalUsed, placementPermits, orderState, cancelWindowOpen,
          lastPlacePermit, lastCancelPermit, halted>>

PlaceAllowed(m, risk, fresh, exact, postSafe, used) ==
    /\ risk
    /\ fresh
    /\ exact
    /\ ~used
    /\ \/ m = "Normal"
       \/ /\ m = "PostOnly"
          /\ postSafe

Init ==
    /\ mode = "Unknown"
    /\ riskApproved = FALSE
    /\ policyFresh = FALSE
    /\ exactOrder = FALSE
    /\ postOnlySafe = FALSE
    /\ approvalUsed = FALSE
    /\ placementPermits = 0
    /\ orderState = "Absent"
    /\ cancelWindowOpen = FALSE
    /\ lastPlacePermit = FALSE
    /\ lastCancelPermit = FALSE
    /\ halted = FALSE

EvaluatePlace(m, risk, fresh, exact, postSafe) ==
    /\ ~halted
    /\ m \in Modes
    /\ risk \in BOOLEAN
    /\ fresh \in BOOLEAN
    /\ exact \in BOOLEAN
    /\ postSafe \in BOOLEAN
    /\ mode' = m
    /\ riskApproved' = risk
    /\ policyFresh' = fresh
    /\ exactOrder' = exact
    /\ postOnlySafe' = postSafe
    /\ lastPlacePermit' = PlaceAllowed(m, risk, fresh, exact, postSafe, approvalUsed)
    /\ IF lastPlacePermit'
          THEN /\ approvalUsed' = TRUE
               /\ placementPermits' = placementPermits + 1
               /\ orderState' = "Authorized"
          ELSE /\ UNCHANGED <<approvalUsed, placementPermits, orderState>>
    /\ lastCancelPermit' = FALSE
    /\ UNCHANGED <<cancelWindowOpen, halted>>

SetDelayed(windowOpen) ==
    /\ ~halted
    /\ orderState = "Authorized"
    /\ windowOpen \in BOOLEAN
    /\ orderState' = "Delayed"
    /\ cancelWindowOpen' = windowOpen
    /\ lastPlacePermit' = FALSE
    /\ lastCancelPermit' = FALSE
    /\ UNCHANGED <<mode, riskApproved, policyFresh, exactOrder, postOnlySafe,
                    approvalUsed, placementPermits, halted>>

EvaluateCancel(m) ==
    /\ ~halted
    /\ m \in Modes
    /\ mode' = m
    /\ lastPlacePermit' = FALSE
    /\ lastCancelPermit' =
          /\ orderState \in {"Authorized", "Delayed", "Live"}
          /\ m \in CancelModes
          /\ orderState /= "Delayed" \/ cancelWindowOpen
    /\ IF lastCancelPermit'
          THEN orderState' = "CancelAuthorized"
          ELSE UNCHANGED orderState
    /\ UNCHANGED <<riskApproved, policyFresh, exactOrder, postOnlySafe,
                    approvalUsed, placementPermits, cancelWindowOpen,
                    halted>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ lastPlacePermit' = FALSE
    /\ lastCancelPermit' = FALSE
    /\ UNCHANGED <<mode, riskApproved, policyFresh, exactOrder, postOnlySafe,
                    approvalUsed, placementPermits, orderState, cancelWindowOpen>>

Halted ==
    /\ halted
    /\ UNCHANGED vars

Next ==
    \/ \E m \in Modes, risk \in BOOLEAN, fresh \in BOOLEAN,
          exact \in BOOLEAN, postSafe \in BOOLEAN:
          EvaluatePlace(m, risk, fresh, exact, postSafe)
    \/ \E windowOpen \in BOOLEAN: SetDelayed(windowOpen)
    \/ \E m \in Modes: EvaluateCancel(m)
    \/ IntegrityFailure
    \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ mode \in Modes
    /\ riskApproved \in BOOLEAN
    /\ policyFresh \in BOOLEAN
    /\ exactOrder \in BOOLEAN
    /\ postOnlySafe \in BOOLEAN
    /\ approvalUsed \in BOOLEAN
    /\ placementPermits \in Nat
    /\ orderState \in {"Absent", "Authorized", "Delayed", "Live",
                         "CancelAuthorized", "Terminal"}
    /\ cancelWindowOpen \in BOOLEAN
    /\ lastPlacePermit \in BOOLEAN
    /\ lastCancelPermit \in BOOLEAN
    /\ halted \in BOOLEAN

PlacePermitRequiresRisk == lastPlacePermit => riskApproved
PlacePermitRequiresPolicy == lastPlacePermit => policyFresh /\ exactOrder
PlacePermitRequiresMode ==
    lastPlacePermit => mode = "Normal" \/ (mode = "PostOnly" /\ postOnlySafe)
ApprovalCannotReplay == placementPermits <= 1
CancelPermitRequiresSafeMode == lastCancelPermit => mode \in CancelModes
CancelPermitRespectsWindow ==
    lastCancelPermit => orderState = "CancelAuthorized"
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
