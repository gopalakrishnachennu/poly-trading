--------------------- MODULE CredentialBrokerSimulator ---------------------
EXTENDS Naturals, TLC

CONSTANT FixtureCount, RequestCount
VARIABLES upstreamValid, keySafe, policySafe, registered, fixtures,
          securityApproved, operationsApproved, securityOperator,
          operationsOperator, permitActive, consumed, revoked, finalized,
          keyMaterial, signatureProduced, providerContacted,
          authenticationAuthority, submissionAuthority, halted

vars == <<upstreamValid, keySafe, policySafe, registered, fixtures,
          securityApproved, operationsApproved, securityOperator,
          operationsOperator, permitActive, consumed, revoked, finalized,
          keyMaterial, signatureProduced, providerContacted,
          authenticationAuthority, submissionAuthority, halted>>

Init == /\ upstreamValid = FALSE /\ keySafe = FALSE /\ policySafe = FALSE
    /\ registered = FALSE /\ fixtures = 0
    /\ securityApproved = FALSE /\ operationsApproved = FALSE
    /\ securityOperator = 0 /\ operationsOperator = 0
    /\ permitActive = FALSE /\ consumed = 0 /\ revoked = FALSE
    /\ finalized = FALSE /\ keyMaterial = FALSE
    /\ signatureProduced = FALSE /\ providerContacted = FALSE
    /\ authenticationAuthority = FALSE /\ submissionAuthority = FALSE
    /\ halted = FALSE

ValidateUpstream == /\ ~halted /\ ~registered /\ upstreamValid' = TRUE
    /\ UNCHANGED <<keySafe, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

ValidateKey == /\ ~halted /\ ~registered /\ keySafe' = TRUE
    /\ UNCHANGED <<upstreamValid, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

ValidatePolicy == /\ ~halted /\ ~registered /\ policySafe' = TRUE
    /\ UNCHANGED <<upstreamValid, keySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

Register == /\ ~halted /\ ~registered
    /\ upstreamValid /\ keySafe /\ policySafe /\ registered' = TRUE
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

RecordFixture == /\ ~halted /\ registered /\ ~finalized
    /\ fixtures < FixtureCount /\ fixtures' = fixtures + 1
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

AuthorizeSecurity == /\ ~halted /\ registered /\ fixtures = FixtureCount
    /\ ~revoked /\ ~finalized /\ consumed < RequestCount
    /\ ~permitActive /\ ~securityApproved
    /\ securityApproved' = TRUE /\ securityOperator' = 1
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    operationsApproved, operationsOperator, permitActive,
                    consumed, revoked, finalized, keyMaterial,
                    signatureProduced, providerContacted,
                    authenticationAuthority, submissionAuthority, halted>>

AuthorizeOperations == /\ ~halted /\ registered /\ fixtures = FixtureCount
    /\ ~revoked /\ ~finalized /\ consumed < RequestCount
    /\ ~permitActive /\ ~operationsApproved
    /\ operationsApproved' = TRUE /\ operationsOperator' = 2
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    securityApproved, securityOperator, permitActive,
                    consumed, revoked, finalized, keyMaterial,
                    signatureProduced, providerContacted,
                    authenticationAuthority, submissionAuthority, halted>>

IssuePermit == /\ ~halted /\ registered /\ fixtures = FixtureCount
    /\ ~revoked /\ ~finalized /\ ~permitActive
    /\ securityApproved /\ operationsApproved
    /\ securityOperator # operationsOperator /\ permitActive' = TRUE
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, consumed, revoked, finalized,
                    keyMaterial, signatureProduced, providerContacted,
                    authenticationAuthority, submissionAuthority, halted>>

ConsumePermit == /\ ~halted /\ permitActive /\ ~revoked /\ ~finalized
    /\ consumed < RequestCount /\ consumed' = consumed + 1
    /\ permitActive' = FALSE /\ securityApproved' = FALSE
    /\ operationsApproved' = FALSE /\ securityOperator' = 0
    /\ operationsOperator' = 0
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    revoked, finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority, halted>>

Revoke == /\ ~halted /\ registered /\ ~revoked /\ ~finalized
    /\ revoked' = TRUE /\ permitActive' = FALSE
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, consumed, finalized, keyMaterial,
                    signatureProduced, providerContacted,
                    authenticationAuthority, submissionAuthority, halted>>

Finalize == /\ ~halted /\ registered /\ ~finalized /\ ~permitActive
    /\ fixtures = FixtureCount /\ (consumed = RequestCount \/ revoked)
    /\ finalized' = TRUE
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    keyMaterial, signatureProduced, providerContacted,
                    authenticationAuthority, submissionAuthority, halted>>

IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<upstreamValid, keySafe, policySafe, registered, fixtures,
                    securityApproved, operationsApproved, securityOperator,
                    operationsOperator, permitActive, consumed, revoked,
                    finalized, keyMaterial, signatureProduced,
                    providerContacted, authenticationAuthority,
                    submissionAuthority>>

Stopped == (halted \/ finalized) /\ UNCHANGED vars

Next == ValidateUpstream \/ ValidateKey \/ ValidatePolicy \/ Register
    \/ RecordFixture \/ AuthorizeSecurity \/ AuthorizeOperations
    \/ IssuePermit \/ ConsumePermit \/ Revoke \/ Finalize
    \/ IntegrityFailure \/ Stopped

Spec == Init /\ [][Next]_vars

TypeOK == /\ upstreamValid \in BOOLEAN /\ keySafe \in BOOLEAN
    /\ policySafe \in BOOLEAN /\ registered \in BOOLEAN
    /\ fixtures \in 0..FixtureCount /\ securityApproved \in BOOLEAN
    /\ operationsApproved \in BOOLEAN /\ securityOperator \in 0..2
    /\ operationsOperator \in 0..2 /\ permitActive \in BOOLEAN
    /\ consumed \in 0..RequestCount /\ revoked \in BOOLEAN
    /\ finalized \in BOOLEAN /\ keyMaterial \in BOOLEAN
    /\ signatureProduced \in BOOLEAN /\ providerContacted \in BOOLEAN
    /\ authenticationAuthority \in BOOLEAN /\ submissionAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegistrationRequiresValidatedInputs ==
    registered => (upstreamValid /\ keySafe /\ policySafe)

PermitRequiresCompleteFixturesAndDualControl ==
    permitActive => (registered /\ fixtures = FixtureCount /\ ~revoked
        /\ securityApproved /\ operationsApproved
        /\ securityOperator # operationsOperator)

RevocationClearsPermit == revoked => ~permitActive

FinalizationRequiresCompletionOrRevocation ==
    finalized => (fixtures = FixtureCount /\ (consumed = RequestCount \/ revoked))

NoSecretOrExternalAuthority ==
    ~keyMaterial /\ ~signatureProduced /\ ~providerContacted
        /\ ~authenticationAuthority /\ ~submissionAuthority
=============================================================================
