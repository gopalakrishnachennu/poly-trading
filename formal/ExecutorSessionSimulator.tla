-------------------- MODULE ExecutorSessionSimulator --------------------
EXTENDS Naturals, TLC

CONSTANT RequestCount
VARIABLES upstreamValid, isolationSafe, registered, leaseActive, sessionOpen,
          requestActive, unknown, reconciliationRequired, restartRequired,
          resolved, closed, finalized, credentialAccess, signatureAccess,
          networkAccess, externalSubmission, externalMutation, halted

vars == <<upstreamValid, isolationSafe, registered, leaseActive, sessionOpen,
          requestActive, unknown, reconciliationRequired, restartRequired,
          resolved, closed, finalized, credentialAccess, signatureAccess,
          networkAccess, externalSubmission, externalMutation, halted>>

Init == /\ upstreamValid = FALSE /\ isolationSafe = FALSE /\ registered = FALSE
    /\ leaseActive = FALSE /\ sessionOpen = FALSE /\ requestActive = FALSE
    /\ unknown = FALSE /\ reconciliationRequired = FALSE
    /\ restartRequired = FALSE /\ resolved = 0 /\ closed = FALSE
    /\ finalized = FALSE /\ credentialAccess = FALSE /\ signatureAccess = FALSE
    /\ networkAccess = FALSE /\ externalSubmission = FALSE
    /\ externalMutation = FALSE /\ halted = FALSE

ValidateUpstream == /\ ~halted /\ ~registered /\ upstreamValid' = TRUE
    /\ UNCHANGED <<isolationSafe, registered, leaseActive, sessionOpen,
                    requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
ValidateIsolation == /\ ~halted /\ ~registered /\ isolationSafe' = TRUE
    /\ UNCHANGED <<upstreamValid, registered, leaseActive, sessionOpen,
                    requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
Register == /\ ~halted /\ ~registered /\ upstreamValid /\ isolationSafe
    /\ registered' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, leaseActive, sessionOpen,
                    requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
AcquireLease == /\ ~halted /\ registered /\ ~leaseActive /\ ~closed
    /\ ~reconciliationRequired /\ ~restartRequired /\ leaseActive' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, sessionOpen,
                    requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
OpenSession == /\ ~halted /\ registered /\ leaseActive /\ ~sessionOpen
    /\ sessionOpen' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
Issue == /\ ~halted /\ sessionOpen /\ leaseActive /\ ~requestActive
    /\ ~reconciliationRequired /\ ~restartRequired /\ resolved < RequestCount
    /\ requestActive' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
ObserveTerminal == /\ ~halted /\ requestActive /\ requestActive' = FALSE
    /\ resolved' = resolved + 1
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, unknown, reconciliationRequired,
                    restartRequired, closed, finalized, credentialAccess,
                    signatureAccess, networkAccess, externalSubmission,
                    externalMutation, halted>>
ObserveUnknown == /\ ~halted /\ requestActive /\ unknown' = TRUE
    /\ reconciliationRequired' = TRUE /\ leaseActive' = FALSE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, sessionOpen,
                    requestActive, restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
DeadMan == /\ ~halted /\ leaseActive /\ leaseActive' = FALSE
    /\ reconciliationRequired' = requestActive
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, sessionOpen,
                    requestActive, unknown, restartRequired, resolved, closed,
                    finalized, credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation, halted>>
Restart == /\ ~halted /\ sessionOpen /\ restartRequired' = TRUE
    /\ leaseActive' = FALSE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, sessionOpen,
                    requestActive, unknown, reconciliationRequired, resolved,
                    closed, finalized, credentialAccess, signatureAccess,
                    networkAccess, externalSubmission, externalMutation, halted>>
Reconcile == /\ ~halted /\ (reconciliationRequired \/ restartRequired)
    /\ reconciliationRequired' = FALSE /\ restartRequired' = FALSE
    /\ unknown' = FALSE /\ requestActive' = FALSE
    /\ resolved' = (IF requestActive THEN resolved + 1 ELSE resolved)
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, closed, finalized, credentialAccess,
                    signatureAccess, networkAccess, externalSubmission,
                    externalMutation, halted>>
Close == /\ ~halted /\ sessionOpen /\ ~requestActive /\ ~leaseActive
    /\ ~reconciliationRequired /\ ~restartRequired /\ resolved = RequestCount
    /\ closed' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, finalized, credentialAccess,
                    signatureAccess, networkAccess, externalSubmission,
                    externalMutation, halted>>
Finalize == /\ ~halted /\ closed /\ ~finalized /\ finalized' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, credentialAccess,
                    signatureAccess, networkAccess, externalSubmission,
                    externalMutation, halted>>
IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<upstreamValid, isolationSafe, registered, leaseActive,
                    sessionOpen, requestActive, unknown, reconciliationRequired,
                    restartRequired, resolved, closed, finalized,
                    credentialAccess, signatureAccess, networkAccess,
                    externalSubmission, externalMutation>>
Stopped == (halted \/ finalized) /\ UNCHANGED vars
Next == ValidateUpstream \/ ValidateIsolation \/ Register \/ AcquireLease
    \/ OpenSession \/ Issue \/ ObserveTerminal \/ ObserveUnknown \/ DeadMan
    \/ Restart \/ Reconcile \/ Close \/ Finalize \/ IntegrityFailure \/ Stopped
Spec == Init /\ [][Next]_vars

TypeOK == /\ upstreamValid \in BOOLEAN /\ isolationSafe \in BOOLEAN
    /\ registered \in BOOLEAN /\ leaseActive \in BOOLEAN
    /\ sessionOpen \in BOOLEAN /\ requestActive \in BOOLEAN
    /\ unknown \in BOOLEAN /\ reconciliationRequired \in BOOLEAN
    /\ restartRequired \in BOOLEAN /\ resolved \in 0..RequestCount
    /\ closed \in BOOLEAN /\ finalized \in BOOLEAN
    /\ credentialAccess \in BOOLEAN /\ signatureAccess \in BOOLEAN
    /\ networkAccess \in BOOLEAN /\ externalSubmission \in BOOLEAN
    /\ externalMutation \in BOOLEAN /\ halted \in BOOLEAN
RegisteredRequiresBindings == registered => (upstreamValid /\ isolationSafe)
RequestRequiresLease == requestActive => sessionOpen
UnknownRequiresReconciliation == unknown => reconciliationRequired
FinalRequiresResolution == finalized => (closed /\ resolved = RequestCount /\ ~requestActive /\ ~leaseActive)
NoExternalAuthority == ~credentialAccess /\ ~signatureAccess /\ ~networkAccess /\ ~externalSubmission /\ ~externalMutation
=============================================================================
