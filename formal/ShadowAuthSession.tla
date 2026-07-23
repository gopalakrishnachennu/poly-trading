------------------------ MODULE ShadowAuthSession ------------------------
EXTENDS Naturals, TLC

CONSTANT MaxEpoch, MaxHeartbeat

VARIABLES upstreamValid, attestationEpoch, registered, leaseActive,
          heartbeatSequence, recoveryReason, cleanCovered, rotationCovered,
          deadmanCovered, restartCovered, ambiguityCovered, finalized,
          credentialLoaded, signatureProduced, providerContacted, socketOpened,
          authenticationAuthority, submissionAuthority, halted

vars == <<upstreamValid, attestationEpoch, registered, leaseActive,
          heartbeatSequence, recoveryReason, cleanCovered, rotationCovered,
          deadmanCovered, restartCovered, ambiguityCovered, finalized,
          credentialLoaded, signatureProduced, providerContacted, socketOpened,
          authenticationAuthority, submissionAuthority, halted>>

Init == /\ upstreamValid = FALSE /\ attestationEpoch = 0
    /\ registered = FALSE /\ leaseActive = FALSE /\ heartbeatSequence = 0
    /\ recoveryReason = 0 /\ cleanCovered = FALSE /\ rotationCovered = FALSE
    /\ deadmanCovered = FALSE /\ restartCovered = FALSE
    /\ ambiguityCovered = FALSE /\ finalized = FALSE
    /\ credentialLoaded = FALSE /\ signatureProduced = FALSE
    /\ providerContacted = FALSE /\ socketOpened = FALSE
    /\ authenticationAuthority = FALSE /\ submissionAuthority = FALSE
    /\ halted = FALSE

ValidateUpstream == /\ ~halted /\ ~registered /\ upstreamValid' = TRUE
    /\ UNCHANGED <<attestationEpoch, registered, leaseActive,
                    heartbeatSequence, recoveryReason, cleanCovered,
                    rotationCovered, deadmanCovered, restartCovered,
                    ambiguityCovered, finalized, credentialLoaded,
                    signatureProduced, providerContacted, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

Register == /\ ~halted /\ ~registered /\ upstreamValid /\ registered' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, leaseActive,
                    heartbeatSequence, recoveryReason, cleanCovered,
                    rotationCovered, deadmanCovered, restartCovered,
                    ambiguityCovered, finalized, credentialLoaded,
                    signatureProduced, providerContacted, socketOpened,
                    authenticationAuthority, submissionAuthority, halted>>

OpenLease == /\ ~halted /\ registered /\ ~finalized
    /\ ~leaseActive /\ recoveryReason = 0 /\ leaseActive' = TRUE
    /\ heartbeatSequence' = 0
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered,
                    recoveryReason, cleanCovered, rotationCovered,
                    deadmanCovered, restartCovered, ambiguityCovered,
                    finalized, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Heartbeat == /\ ~halted /\ leaseActive /\ recoveryReason = 0
    /\ heartbeatSequence < MaxHeartbeat
    /\ heartbeatSequence' = heartbeatSequence + 1
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered, leaseActive,
                    recoveryReason, cleanCovered, rotationCovered,
                    deadmanCovered, restartCovered, ambiguityCovered,
                    finalized, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

CloseLease == /\ ~halted /\ leaseActive /\ recoveryReason = 0
    /\ leaseActive' = FALSE /\ cleanCovered' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered,
                    heartbeatSequence, recoveryReason, rotationCovered,
                    deadmanCovered, restartCovered, ambiguityCovered,
                    finalized, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Rotate == /\ ~halted /\ registered /\ ~leaseActive
    /\ recoveryReason = 0 /\ ~finalized
    /\ attestationEpoch < MaxEpoch
    /\ attestationEpoch' = attestationEpoch + 1 /\ rotationCovered' = TRUE
    /\ UNCHANGED <<upstreamValid, registered, leaseActive,
                    heartbeatSequence, recoveryReason, cleanCovered,
                    deadmanCovered, restartCovered, ambiguityCovered,
                    finalized, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

DeadMan == /\ ~halted /\ leaseActive /\ recoveryReason = 0
    /\ leaseActive' = FALSE /\ recoveryReason' = 1 /\ deadmanCovered' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered,
                    heartbeatSequence, cleanCovered, rotationCovered,
                    restartCovered, ambiguityCovered, finalized,
                    credentialLoaded, signatureProduced, providerContacted,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Restart == /\ ~halted /\ leaseActive /\ recoveryReason = 0
    /\ leaseActive' = FALSE /\ recoveryReason' = 2 /\ restartCovered' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered,
                    heartbeatSequence, cleanCovered, rotationCovered,
                    deadmanCovered, ambiguityCovered, finalized,
                    credentialLoaded, signatureProduced, providerContacted,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Ambiguity == /\ ~halted /\ leaseActive /\ recoveryReason = 0
    /\ leaseActive' = FALSE /\ recoveryReason' = 3
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered,
                    heartbeatSequence, cleanCovered, rotationCovered,
                    deadmanCovered, restartCovered, ambiguityCovered,
                    finalized, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Recover == /\ ~halted /\ ~leaseActive /\ recoveryReason # 0
    /\ ambiguityCovered' = (ambiguityCovered \/ recoveryReason = 3)
    /\ recoveryReason' = 0
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered, leaseActive,
                    heartbeatSequence, cleanCovered, rotationCovered,
                    deadmanCovered, restartCovered, finalized,
                    credentialLoaded, signatureProduced, providerContacted,
                    socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

Finalize == /\ ~halted /\ registered /\ ~leaseActive /\ recoveryReason = 0
    /\ cleanCovered /\ rotationCovered /\ deadmanCovered
    /\ restartCovered /\ ambiguityCovered /\ ~finalized /\ finalized' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered, leaseActive,
                    heartbeatSequence, recoveryReason, cleanCovered,
                    rotationCovered, deadmanCovered, restartCovered,
                    ambiguityCovered, credentialLoaded, signatureProduced,
                    providerContacted, socketOpened, authenticationAuthority,
                    submissionAuthority, halted>>

IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<upstreamValid, attestationEpoch, registered, leaseActive,
                    heartbeatSequence, recoveryReason, cleanCovered,
                    rotationCovered, deadmanCovered, restartCovered,
                    ambiguityCovered, finalized, credentialLoaded,
                    signatureProduced, providerContacted, socketOpened,
                    authenticationAuthority, submissionAuthority>>

Stopped == (halted \/ finalized) /\ UNCHANGED vars

Next == ValidateUpstream \/ Register \/ OpenLease \/ Heartbeat \/ CloseLease
    \/ Rotate \/ DeadMan \/ Restart \/ Ambiguity \/ Recover \/ Finalize
    \/ IntegrityFailure \/ Stopped

Spec == Init /\ [][Next]_vars

TypeOK == /\ upstreamValid \in BOOLEAN /\ attestationEpoch \in 0..MaxEpoch
    /\ registered \in BOOLEAN /\ leaseActive \in BOOLEAN
    /\ heartbeatSequence \in 0..MaxHeartbeat /\ recoveryReason \in 0..3
    /\ cleanCovered \in BOOLEAN /\ rotationCovered \in BOOLEAN
    /\ deadmanCovered \in BOOLEAN /\ restartCovered \in BOOLEAN
    /\ ambiguityCovered \in BOOLEAN /\ finalized \in BOOLEAN
    /\ credentialLoaded \in BOOLEAN /\ signatureProduced \in BOOLEAN
    /\ providerContacted \in BOOLEAN /\ socketOpened \in BOOLEAN
    /\ authenticationAuthority \in BOOLEAN /\ submissionAuthority \in BOOLEAN
    /\ halted \in BOOLEAN

RegistrationRequiresCertification == registered => upstreamValid

RecoveryRevokesLease == recoveryReason # 0 => ~leaseActive

FinalizationRequiresCoverage == finalized =>
    (~leaseActive /\ recoveryReason = 0 /\ cleanCovered /\ rotationCovered
        /\ deadmanCovered /\ restartCovered /\ ambiguityCovered)

NoExternalAuthority == ~credentialLoaded /\ ~signatureProduced
    /\ ~providerContacted /\ ~socketOpened /\ ~authenticationAuthority
    /\ ~submissionAuthority
=============================================================================
