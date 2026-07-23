-------------------- MODULE TransportAdapterCertification --------------------
EXTENDS Naturals, TLC

CONSTANT FixtureCount, UnknownIndex, ReconcileIndex
VARIABLES upstreamValid, endpointSafe, bindingsComplete, registered, fixtures,
          unknownPending, finalized, certified, socketOpened, credentialLoaded,
          signatureProduced, authenticatedRequest, externalSubmission,
          externalMutation, halted

vars == <<upstreamValid, endpointSafe, bindingsComplete, registered, fixtures,
          unknownPending, finalized, certified, socketOpened, credentialLoaded,
          signatureProduced, authenticatedRequest, externalSubmission,
          externalMutation, halted>>

Init == /\ upstreamValid = FALSE /\ endpointSafe = FALSE
    /\ bindingsComplete = FALSE /\ registered = FALSE /\ fixtures = 0
    /\ unknownPending = FALSE /\ finalized = FALSE /\ certified = FALSE
    /\ socketOpened = FALSE /\ credentialLoaded = FALSE
    /\ signatureProduced = FALSE /\ authenticatedRequest = FALSE
    /\ externalSubmission = FALSE /\ externalMutation = FALSE /\ halted = FALSE

ValidateUpstream == /\ ~halted /\ ~registered /\ upstreamValid' = TRUE
    /\ UNCHANGED <<endpointSafe, bindingsComplete, registered, fixtures,
                    unknownPending, finalized, certified, socketOpened,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
ValidateEndpoint == /\ ~halted /\ ~registered /\ endpointSafe' = TRUE
    /\ UNCHANGED <<upstreamValid, bindingsComplete, registered, fixtures,
                    unknownPending, finalized, certified, socketOpened,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
ValidateBindings == /\ ~halted /\ ~registered /\ bindingsComplete' = TRUE
    /\ UNCHANGED <<upstreamValid, endpointSafe, registered, fixtures,
                    unknownPending, finalized, certified, socketOpened,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
Register == /\ ~halted /\ ~registered /\ upstreamValid /\ endpointSafe
    /\ bindingsComplete /\ registered' = TRUE
    /\ UNCHANGED <<upstreamValid, endpointSafe, bindingsComplete, fixtures,
                    unknownPending, finalized, certified, socketOpened,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
RecordFixture == /\ ~halted /\ registered /\ ~finalized
    /\ fixtures < FixtureCount /\ ~unknownPending
    /\ fixtures' = fixtures + 1
    /\ unknownPending' = (fixtures = UnknownIndex)
    /\ UNCHANGED <<upstreamValid, endpointSafe, bindingsComplete, registered,
                    finalized, certified, socketOpened, credentialLoaded,
                    signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
RecordReconciliation == /\ ~halted /\ registered /\ ~finalized
    /\ unknownPending /\ fixtures = ReconcileIndex
    /\ fixtures' = fixtures + 1 /\ unknownPending' = FALSE
    /\ UNCHANGED <<upstreamValid, endpointSafe, bindingsComplete, registered,
                    finalized, certified, socketOpened, credentialLoaded,
                    signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
Finalize == /\ ~halted /\ registered /\ ~finalized
    /\ fixtures = FixtureCount /\ ~unknownPending
    /\ finalized' = TRUE /\ certified' = TRUE
    /\ UNCHANGED <<upstreamValid, endpointSafe, bindingsComplete, registered,
                    fixtures, unknownPending, socketOpened, credentialLoaded,
                    signatureProduced, authenticatedRequest,
                    externalSubmission, externalMutation, halted>>
IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<upstreamValid, endpointSafe, bindingsComplete, registered,
                    fixtures, unknownPending, finalized, certified,
                    socketOpened, credentialLoaded, signatureProduced,
                    authenticatedRequest, externalSubmission, externalMutation>>
Stopped == (halted \/ finalized) /\ UNCHANGED vars
Next == ValidateUpstream \/ ValidateEndpoint \/ ValidateBindings \/ Register
    \/ RecordFixture \/ RecordReconciliation \/ Finalize
    \/ IntegrityFailure \/ Stopped
Spec == Init /\ [][Next]_vars

TypeOK == /\ upstreamValid \in BOOLEAN /\ endpointSafe \in BOOLEAN
    /\ bindingsComplete \in BOOLEAN /\ registered \in BOOLEAN
    /\ fixtures \in 0..FixtureCount /\ unknownPending \in BOOLEAN
    /\ finalized \in BOOLEAN /\ certified \in BOOLEAN
    /\ socketOpened \in BOOLEAN /\ credentialLoaded \in BOOLEAN
    /\ signatureProduced \in BOOLEAN /\ authenticatedRequest \in BOOLEAN
    /\ externalSubmission \in BOOLEAN /\ externalMutation \in BOOLEAN
    /\ halted \in BOOLEAN
RegistrationRequiresAllBindings == registered => (upstreamValid /\ endpointSafe /\ bindingsComplete)
UnknownBlocksProgress == unknownPending => fixtures = ReconcileIndex
CertificationRequiresCompleteMatrix == certified => (finalized /\ fixtures = FixtureCount /\ ~unknownPending)
NoExternalAuthority == ~socketOpened /\ ~credentialLoaded /\ ~signatureProduced /\ ~authenticatedRequest /\ ~externalSubmission /\ ~externalMutation
=============================================================================
