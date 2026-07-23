-------------------- MODULE DeploymentExecutionIntent --------------------
EXTENDS Naturals, TLC

CONSTANT DryRunCount, StepCount
VARIABLES readinessValid, subjectBound, contractSafe, registered, dryRuns,
          certified, issued, consumed, stepsDone, finalized,
          credentialLoaded, signatureProduced, authenticatedRequest,
          externalMutation, deploymentAuthority, halted

vars == <<readinessValid, subjectBound, contractSafe, registered, dryRuns,
          certified, issued, consumed, stepsDone, finalized,
          credentialLoaded, signatureProduced, authenticatedRequest,
          externalMutation, deploymentAuthority, halted>>

Init ==
    /\ readinessValid = FALSE /\ subjectBound = FALSE /\ contractSafe = FALSE
    /\ registered = FALSE /\ dryRuns = 0 /\ certified = FALSE
    /\ issued = FALSE /\ consumed = FALSE /\ stepsDone = 0
    /\ finalized = FALSE /\ credentialLoaded = FALSE
    /\ signatureProduced = FALSE /\ authenticatedRequest = FALSE
    /\ externalMutation = FALSE /\ deploymentAuthority = FALSE /\ halted = FALSE

ValidateReadiness == /\ ~halted /\ ~registered /\ readinessValid' = TRUE
    /\ UNCHANGED <<subjectBound, contractSafe, registered, dryRuns, certified,
                    issued, consumed, stepsDone, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
BindSubject == /\ ~halted /\ ~registered /\ subjectBound' = TRUE
    /\ UNCHANGED <<readinessValid, contractSafe, registered, dryRuns, certified,
                    issued, consumed, stepsDone, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
ValidateContract == /\ ~halted /\ ~registered /\ contractSafe' = TRUE
    /\ UNCHANGED <<readinessValid, subjectBound, registered, dryRuns, certified,
                    issued, consumed, stepsDone, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
Register == /\ ~halted /\ ~registered /\ readinessValid /\ subjectBound /\ contractSafe
    /\ registered' = TRUE
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, dryRuns, certified,
                    issued, consumed, stepsDone, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
RecordDryRun == /\ ~halted /\ registered /\ ~certified /\ dryRuns < DryRunCount
    /\ dryRuns' = dryRuns + 1
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    certified, issued, consumed, stepsDone, finalized,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalMutation, deploymentAuthority, halted>>
Certify == /\ ~halted /\ registered /\ dryRuns = DryRunCount /\ ~certified
    /\ certified' = TRUE
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    dryRuns, issued, consumed, stepsDone, finalized,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalMutation, deploymentAuthority, halted>>
Issue == /\ ~halted /\ certified /\ ~issued /\ stepsDone < StepCount
    /\ issued' = TRUE /\ consumed' = FALSE
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    dryRuns, certified, stepsDone, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
Consume == /\ ~halted /\ issued /\ ~consumed
    /\ issued' = FALSE /\ consumed' = TRUE /\ stepsDone' = stepsDone + 1
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    dryRuns, certified, finalized, credentialLoaded,
                    signatureProduced, authenticatedRequest, externalMutation,
                    deploymentAuthority, halted>>
Finalize == /\ ~halted /\ certified /\ ~issued /\ stepsDone = StepCount
    /\ finalized' = TRUE
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    dryRuns, certified, issued, consumed, stepsDone,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalMutation, deploymentAuthority, halted>>
IntegrityFailure == /\ ~halted /\ halted' = TRUE
    /\ UNCHANGED <<readinessValid, subjectBound, contractSafe, registered,
                    dryRuns, certified, issued, consumed, stepsDone, finalized,
                    credentialLoaded, signatureProduced, authenticatedRequest,
                    externalMutation, deploymentAuthority>>
Stopped == (halted \/ finalized) /\ UNCHANGED vars
Next == ValidateReadiness \/ BindSubject \/ ValidateContract \/ Register
        \/ RecordDryRun \/ Certify \/ Issue \/ Consume \/ Finalize
        \/ IntegrityFailure \/ Stopped
Spec == Init /\ [][Next]_vars

TypeOK == /\ readinessValid \in BOOLEAN /\ subjectBound \in BOOLEAN
    /\ contractSafe \in BOOLEAN /\ registered \in BOOLEAN
    /\ dryRuns \in 0..DryRunCount /\ certified \in BOOLEAN
    /\ issued \in BOOLEAN /\ consumed \in BOOLEAN /\ stepsDone \in 0..StepCount
    /\ finalized \in BOOLEAN /\ credentialLoaded \in BOOLEAN
    /\ signatureProduced \in BOOLEAN /\ authenticatedRequest \in BOOLEAN
    /\ externalMutation \in BOOLEAN /\ deploymentAuthority \in BOOLEAN
    /\ halted \in BOOLEAN
RegistrationRequiresAllBindings == registered => (readinessValid /\ subjectBound /\ contractSafe)
CertificationRequiresMatrix == certified => dryRuns = DryRunCount
IntentRequiresCertification == issued => certified
FinalRequiresAllSteps == finalized => (certified /\ ~issued /\ stepsDone = StepCount)
NoAuthority == ~credentialLoaded /\ ~signatureProduced /\ ~authenticatedRequest /\ ~externalMutation /\ ~deploymentAuthority
=============================================================================
