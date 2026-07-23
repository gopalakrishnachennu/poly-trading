--------------------- MODULE AuthenticatedNoSubmit ---------------------
EXTENDS Integers, FiniteSets, TLC
CONSTANT MaxEpoch
Scenarios == {"activation", "observation", "rotation", "revocation", "outage",
              "deadman", "unknown", "dr", "physical", "logical"}
VARIABLES registered, epoch, active, revoked, covered, certified, credential,
          signature, connection, submitPhysical, submitLogical, authority, halted
vars == <<registered, epoch, active, revoked, covered, certified, credential,
          signature, connection, submitPhysical, submitLogical, authority, halted>>
Init == /\ registered=FALSE /\ epoch=0 /\ active=FALSE /\ revoked=FALSE
        /\ covered={} /\ certified=FALSE /\ credential=FALSE /\ signature=FALSE
        /\ connection=FALSE /\ submitPhysical=FALSE /\ submitLogical=FALSE
        /\ authority=FALSE /\ halted=FALSE
Register == /\ ~registered /\ ~halted /\ registered'=TRUE
            /\ UNCHANGED <<epoch,active,revoked,covered,certified,credential,
                           signature,connection,submitPhysical,submitLogical,authority,halted>>
Issue == /\ registered /\ epoch=0 /\ ~halted /\ epoch'=1 /\ active'=TRUE
         /\ UNCHANGED <<registered,revoked,covered,certified,credential,signature,
                        connection,submitPhysical,submitLogical,authority,halted>>
Rotate == /\ active /\ epoch<MaxEpoch /\ ~certified /\ ~halted
          /\ epoch'=epoch+1 /\ covered'=covered\cup{"rotation"}
          /\ UNCHANGED <<registered,active,revoked,certified,credential,signature,
                         connection,submitPhysical,submitLogical,authority,halted>>
Fixture(s) == /\ s\in Scenarios /\ active /\ ~certified /\ ~halted
              /\ covered'=covered\cup{s}
              /\ UNCHANGED <<registered,epoch,active,revoked,certified,credential,
                             signature,connection,submitPhysical,submitLogical,authority,halted>>
Revoke == /\ active /\ ~revoked /\ ~certified /\ ~halted /\ active'=FALSE
          /\ revoked'=TRUE /\ covered'=covered\cup{"revocation"}
          /\ UNCHANGED <<registered,epoch,certified,credential,signature,connection,
                         submitPhysical,submitLogical,authority,halted>>
Finalize == /\ revoked /\ Scenarios\subseteq covered /\ ~certified /\ ~halted
            /\ certified'=TRUE
            /\ UNCHANGED <<registered,epoch,active,revoked,covered,credential,
                           signature,connection,submitPhysical,submitLogical,authority,halted>>
Fail == /\ ~certified /\ ~halted /\ halted'=TRUE
        /\ UNCHANGED <<registered,epoch,active,revoked,covered,certified,credential,
                       signature,connection,submitPhysical,submitLogical,authority>>
TerminalStutter == (certified\/halted)/\UNCHANGED vars
Next == Register \/ Issue \/ Rotate \/ (\E s\in Scenarios:Fixture(s)) \/ Revoke \/ Finalize \/ Fail \/ TerminalStutter
TypeInvariant == /\ registered\in BOOLEAN /\ epoch\in 0..MaxEpoch /\ active\in BOOLEAN
                 /\ revoked\in BOOLEAN /\ covered\subseteq Scenarios /\ certified\in BOOLEAN
                 /\ credential\in BOOLEAN /\ signature\in BOOLEAN /\ connection\in BOOLEAN
                 /\ submitPhysical\in BOOLEAN /\ submitLogical\in BOOLEAN /\ authority\in BOOLEAN /\ halted\in BOOLEAN
RevocationFinal == revoked=>~active
CompleteCertificate == certified=>revoked/\Scenarios\subseteq covered
NoCredential == ~credential
NoSignature == ~signature
NoConnection == ~connection
NoPhysicalSubmit == ~submitPhysical
NoLogicalSubmit == ~submitLogical
NoAuthority == ~authority
HaltNotCertified == halted=>~certified
Spec == Init/\[][Next]_vars
=======================================================================
