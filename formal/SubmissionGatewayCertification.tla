------------------ MODULE SubmissionGatewayCertification ------------------
EXTENDS Naturals, TLC

CONSTANT FixtureCount, RequestCount
VARIABLES upstreamValid, bindingsValid, registered, fixtures, active,
          consumedIdentities, completed, rejected, reconciled, finalized,
          credentialLoaded, signatureProduced, socketOpened,
          authenticationAuthority, submissionAuthority, halted

vars == <<upstreamValid, bindingsValid, registered, fixtures, active,
          consumedIdentities, completed, rejected, reconciled, finalized,
          credentialLoaded, signatureProduced, socketOpened,
          authenticationAuthority, submissionAuthority, halted>>

Init == /\ upstreamValid = FALSE /\ bindingsValid = FALSE
    /\ registered = FALSE /\ fixtures = 0 /\ active = 0
    /\ consumedIdentities = 0 /\ completed = 0 /\ rejected = 0
    /\ reconciled = 0 /\ finalized = FALSE /\ credentialLoaded = FALSE
    /\ signatureProduced = FALSE /\ socketOpened = FALSE
    /\ authenticationAuthority = FALSE /\ submissionAuthority = FALSE
    /\ halted = FALSE

ValidateUpstream == /\ ~halted /\ ~registered /\ upstreamValid' = TRUE
    /\ UNCHANGED <<bindingsValid, registered, fixtures, active,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

ValidateBindings == /\ ~halted /\ ~registered /\ bindingsValid' = TRUE
    /\ UNCHANGED <<upstreamValid, registered, fixtures, active,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Register == /\ ~halted /\ ~registered /\ upstreamValid /\ bindingsValid
    /\ registered' = TRUE
    /\ UNCHANGED <<upstreamValid, bindingsValid, fixtures, active,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

RecordFixture == /\ ~halted /\ registered /\ ~finalized
    /\ fixtures < FixtureCount /\ fixtures' = fixtures + 1
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, active,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Stage == /\ ~halted /\ registered /\ fixtures = FixtureCount
    /\ ~finalized /\ active = 0 /\ completed < RequestCount
    /\ consumedIdentities = completed /\ active' = 1
    /\ consumedIdentities' = consumedIdentities + 1
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures,
                    completed, rejected, reconciled, finalized,
                    credentialLoaded, signatureProduced, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

ObserveAccepted == /\ ~halted /\ active = 1 /\ ~finalized
    /\ active' = 0 /\ completed' = completed + 1
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures,
                    consumedIdentities, rejected, reconciled, finalized,
                    credentialLoaded, signatureProduced, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

ObserveRejected == /\ ~halted /\ active = 1 /\ ~finalized
    /\ active' = 0 /\ completed' = completed + 1
    /\ rejected' = rejected + 1
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures,
                    consumedIdentities, reconciled, finalized,
                    credentialLoaded, signatureProduced, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

ObserveUnknown == /\ ~halted /\ active = 1 /\ ~finalized
    /\ active' = 2
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

ReconcileUnknown == /\ ~halted /\ active = 2 /\ ~finalized
    /\ active' = 0 /\ completed' = completed + 1
    /\ reconciled' = reconciled + 1
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures,
                    consumedIdentities, rejected, finalized,
                    credentialLoaded, signatureProduced, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

Finalize == /\ ~halted /\ registered /\ ~finalized /\ active = 0
    /\ fixtures = FixtureCount /\ completed = RequestCount
    /\ finalized' = TRUE
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures, active,
                    consumedIdentities, completed, rejected, reconciled,
                    credentialLoaded, signatureProduced, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<upstreamValid, bindingsValid, registered, fixtures, active,
                    consumedIdentities, completed, rejected, reconciled,
                    finalized, credentialLoaded, signatureProduced,
                    socketOpened, authenticationAuthority, submissionAuthority>>

Stopped == (halted \/ finalized) /\ UNCHANGED vars

Next == ValidateUpstream \/ ValidateBindings \/ Register \/ RecordFixture
    \/ Stage \/ ObserveAccepted \/ ObserveRejected \/ ObserveUnknown
    \/ ReconcileUnknown \/ Finalize \/ IntegrityFailure \/ Stopped

Spec == Init /\ [][Next]_vars

TypeOK == /\ upstreamValid \in BOOLEAN /\ bindingsValid \in BOOLEAN
    /\ registered \in BOOLEAN /\ fixtures \in 0..FixtureCount
    /\ active \in 0..2 /\ consumedIdentities \in 0..RequestCount
    /\ completed \in 0..RequestCount /\ rejected \in 0..RequestCount
    /\ reconciled \in 0..RequestCount /\ finalized \in BOOLEAN
    /\ credentialLoaded \in BOOLEAN /\ signatureProduced \in BOOLEAN
    /\ socketOpened \in BOOLEAN /\ authenticationAuthority \in BOOLEAN
    /\ submissionAuthority \in BOOLEAN /\ halted \in BOOLEAN

RegistrationRequiresExactEvidence == registered => (upstreamValid /\ bindingsValid)

SubmissionRequiresFixtures == active # 0 => (registered /\ fixtures = FixtureCount)

ExactlyOnceIdentities == completed <= consumedIdentities

UnknownBlocksNewStage == active = 2 => consumedIdentities = completed + 1

FinalizationRequiresResolution ==
    finalized => (active = 0 /\ completed = RequestCount /\ fixtures = FixtureCount)

NoAuthenticationOrSubmissionAuthority ==
    ~credentialLoaded /\ ~signatureProduced /\ ~socketOpened
        /\ ~authenticationAuthority /\ ~submissionAuthority
=============================================================================
