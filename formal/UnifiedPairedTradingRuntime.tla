---------------- MODULE UnifiedPairedTradingRuntime ----------------
EXTENDS Naturals, TLC

States == {"Empty", "Funded", "Reconciled", "Staged", "FirstSubmitted",
           "FirstFilled", "HedgeSubmitted", "HedgeFilled", "Settled",
           "Locked", "MergePending", "MergeConfirmed", "Finalized"}

VARIABLES state, reservations, submittedLegs, filledLegs, postedLegs,
          pairBacking, mergeConfirmed, noTradeCount, signing, liveSubmission,
          childHalted, halted

vars == <<state, reservations, submittedLegs, filledLegs, postedLegs,
          pairBacking, mergeConfirmed, noTradeCount, signing, liveSubmission,
          childHalted, halted>>

Init ==
    /\ state = "Empty"
    /\ reservations = 0
    /\ submittedLegs = 0
    /\ filledLegs = 0
    /\ postedLegs = 0
    /\ pairBacking = FALSE
    /\ mergeConfirmed = FALSE
    /\ noTradeCount = 0
    /\ signing = FALSE
    /\ liveSubmission = FALSE
    /\ childHalted = FALSE
    /\ halted = FALSE

Fund ==
    /\ ~halted
    /\ state = "Empty"
    /\ state' = "Funded"
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, noTradeCount, signing,
                    liveSubmission, childHalted, halted>>

Reconcile ==
    /\ ~halted
    /\ state = "Funded"
    /\ state' = "Reconciled"
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, noTradeCount, signing,
                    liveSubmission, childHalted, halted>>

NoTrade ==
    /\ ~halted
    /\ state = "Reconciled"
    /\ noTradeCount < 3
    /\ noTradeCount' = noTradeCount + 1
    /\ UNCHANGED <<state, reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, signing, liveSubmission,
                    childHalted, halted>>

Stage ==
    /\ ~halted
    /\ state = "Reconciled"
    /\ state' = "Staged"
    /\ reservations' = 2
    /\ UNCHANGED <<submittedLegs, filledLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

AuthorizeSubmitFirst ==
    /\ ~halted
    /\ state = "Staged"
    /\ state' = "FirstSubmitted"
    /\ submittedLegs' = 1
    /\ UNCHANGED <<reservations, filledLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

FillFirst ==
    /\ ~halted
    /\ state = "FirstSubmitted"
    /\ state' = "FirstFilled"
    /\ filledLegs' = 1
    /\ UNCHANGED <<reservations, submittedLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

AuthorizeSubmitHedge ==
    /\ ~halted
    /\ state = "FirstFilled"
    /\ state' = "HedgeSubmitted"
    /\ submittedLegs' = 2
    /\ UNCHANGED <<reservations, filledLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

FillHedge ==
    /\ ~halted
    /\ state = "HedgeSubmitted"
    /\ state' = "HedgeFilled"
    /\ filledLegs' = 2
    /\ UNCHANGED <<reservations, submittedLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

PostConfirmed ==
    /\ ~halted
    /\ state \in {"HedgeFilled", "Settled"}
    /\ postedLegs < 2
    /\ postedLegs' = postedLegs + 1
    /\ state' = IF postedLegs' = 2 THEN "Settled" ELSE state
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

LockPair ==
    /\ ~halted
    /\ state = "Settled"
    /\ postedLegs = 2
    /\ state' = "Locked"
    /\ pairBacking' = TRUE
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, postedLegs,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

RequestMerge ==
    /\ ~halted
    /\ state = "Locked"
    /\ pairBacking
    /\ state' = "MergePending"
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, noTradeCount, signing,
                    liveSubmission, childHalted, halted>>

ConfirmMerge ==
    /\ ~halted
    /\ state = "MergePending"
    /\ pairBacking
    /\ state' = "MergeConfirmed"
    /\ pairBacking' = FALSE
    /\ mergeConfirmed' = TRUE
    /\ UNCHANGED <<reservations, submittedLegs, filledLegs, postedLegs,
                    noTradeCount, signing, liveSubmission, childHalted, halted>>

Finalize ==
    /\ ~halted
    /\ state = "MergeConfirmed"
    /\ state' = "Finalized"
    /\ reservations' = 0
    /\ UNCHANGED <<submittedLegs, filledLegs, postedLegs, pairBacking,
                    mergeConfirmed, noTradeCount, signing, liveSubmission,
                    childHalted, halted>>

ChildFailure ==
    /\ ~halted
    /\ childHalted' = TRUE
    /\ halted' = TRUE
    /\ UNCHANGED <<state, reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, noTradeCount, signing,
                    liveSubmission>>

IntegrityFailure ==
    /\ ~halted
    /\ halted' = TRUE
    /\ UNCHANGED <<state, reservations, submittedLegs, filledLegs, postedLegs,
                    pairBacking, mergeConfirmed, noTradeCount, signing,
                    liveSubmission, childHalted>>

Halted == halted /\ UNCHANGED vars

Next ==
    \/ Fund \/ Reconcile \/ NoTrade \/ Stage
    \/ AuthorizeSubmitFirst \/ FillFirst \/ AuthorizeSubmitHedge \/ FillHedge
    \/ PostConfirmed \/ LockPair \/ RequestMerge \/ ConfirmMerge \/ Finalize
    \/ ChildFailure \/ IntegrityFailure \/ Halted

Spec == Init /\ [][Next]_vars

TypeInvariant ==
    /\ state \in States
    /\ reservations \in 0..2
    /\ submittedLegs \in 0..2
    /\ filledLegs \in 0..2
    /\ postedLegs \in 0..2
    /\ pairBacking \in BOOLEAN
    /\ mergeConfirmed \in BOOLEAN
    /\ noTradeCount \in 0..3
    /\ signing \in BOOLEAN
    /\ liveSubmission \in BOOLEAN
    /\ childHalted \in BOOLEAN
    /\ halted \in BOOLEAN

OrderedAuthority == postedLegs <= filledLegs /\ filledLegs <= submittedLegs
StagingBacked == state \in {"Staged", "FirstSubmitted", "FirstFilled",
    "HedgeSubmitted", "HedgeFilled", "Settled", "Locked", "MergePending",
    "MergeConfirmed"} => reservations = 2
HedgeAfterFirst == submittedLegs = 2 => filledLegs >= 1
ConfirmedOnlyPosting == postedLegs > 0 => filledLegs = 2
MergeBacked == state = "MergePending" => pairBacking
MergeFinality == mergeConfirmed => state \in {"MergeConfirmed", "Finalized"}
FinalizedReleases == state = "Finalized" => reservations = 0 /\ mergeConfirmed
NoLiveAuthority == ~signing /\ ~liveSubmission
ChildHaltPropagates == childHalted => halted
HaltIsAbsorbing == [](halted => []halted)

=============================================================================
