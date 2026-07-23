---------------- MODULE CredentialProviderCertification ----------------
EXTENDS Integers, FiniteSets, TLC

CONSTANT MaxEpoch

Scenarios == {
  "acquire", "rotate", "revoke", "quota", "outage",
  "attestation", "split_brain", "disaster_recovery", "stale_epoch"
}

VARIABLES registered, epoch, active, revoked, recovery, covered,
          certified, authority, halted

vars == <<registered, epoch, active, revoked, recovery, covered,
          certified, authority, halted>>

Init ==
  /\ registered = FALSE
  /\ epoch = 0
  /\ active = FALSE
  /\ revoked = {}
  /\ recovery = FALSE
  /\ covered = {}
  /\ certified = FALSE
  /\ authority = FALSE
  /\ halted = FALSE

Register ==
  /\ ~registered
  /\ ~halted
  /\ registered' = TRUE
  /\ UNCHANGED <<epoch, active, revoked, recovery, covered,
                 certified, authority, halted>>

Acquire ==
  /\ registered /\ ~active /\ ~recovery /\ ~certified /\ ~halted
  /\ epoch < MaxEpoch
  /\ epoch' = epoch + 1
  /\ active' = TRUE
  /\ covered' = covered \cup {"acquire"}
  /\ UNCHANGED <<registered, revoked, recovery, certified, authority, halted>>

Rotate ==
  /\ registered /\ active /\ ~recovery /\ ~certified /\ ~halted
  /\ epoch < MaxEpoch
  /\ epoch' = epoch + 1
  /\ covered' = covered \cup {"rotate"}
  /\ UNCHANGED <<registered, active, revoked, recovery,
                 certified, authority, halted>>

SafeFixture(s) ==
  /\ s \in {"quota", "outage", "attestation", "stale_epoch"}
  /\ registered /\ ~recovery /\ ~certified /\ ~halted
  /\ covered' = covered \cup {s}
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery,
                 certified, authority, halted>>

SplitBrain ==
  /\ registered /\ active /\ ~recovery /\ ~certified /\ ~halted
  /\ active' = FALSE
  /\ revoked' = revoked \cup {epoch}
  /\ recovery' = TRUE
  /\ covered' = covered \cup {"split_brain"}
  /\ UNCHANGED <<registered, epoch, certified, authority, halted>>

Recover ==
  /\ registered /\ ~active /\ recovery /\ ~certified /\ ~halted
  /\ recovery' = FALSE
  /\ UNCHANGED <<registered, epoch, active, revoked, covered,
                 certified, authority, halted>>

Revoke ==
  /\ registered /\ active /\ ~recovery /\ ~certified /\ ~halted
  /\ active' = FALSE
  /\ revoked' = revoked \cup {epoch}
  /\ covered' = covered \cup {"revoke"}
  /\ UNCHANGED <<registered, epoch, recovery, certified, authority, halted>>

DisasterRecovery ==
  /\ registered /\ ~active /\ ~recovery /\ ~certified /\ ~halted
  /\ covered' = covered \cup {"disaster_recovery"}
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery,
                 certified, authority, halted>>

Finalize ==
  /\ registered /\ ~active /\ ~recovery /\ ~certified /\ ~halted
  /\ Scenarios \subseteq covered
  /\ certified' = TRUE
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery,
                 covered, authority, halted>>

Fail ==
  /\ ~halted /\ ~certified
  /\ halted' = TRUE
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery,
                 covered, certified, authority>>

HaltedStutter == halted /\ UNCHANGED vars
CertifiedStutter == certified /\ UNCHANGED vars

Next ==
  \/ Register
  \/ Acquire
  \/ Rotate
  \/ \E s \in Scenarios : SafeFixture(s)
  \/ SplitBrain
  \/ Recover
  \/ Revoke
  \/ DisasterRecovery
  \/ Finalize
  \/ Fail
  \/ HaltedStutter
  \/ CertifiedStutter

TypeInvariant ==
  /\ registered \in BOOLEAN
  /\ epoch \in 0..MaxEpoch
  /\ active \in BOOLEAN
  /\ revoked \subseteq 0..MaxEpoch
  /\ recovery \in BOOLEAN
  /\ covered \subseteq Scenarios
  /\ certified \in BOOLEAN
  /\ authority \in BOOLEAN
  /\ halted \in BOOLEAN

NoActiveRecovery == ~(active /\ recovery)
RevokedInactive == active => ~(epoch \in revoked)
RecoveryHasRevocation == recovery => Cardinality(revoked) > 0
CertifiedComplete == certified => Scenarios \subseteq covered
CertifiedQuiescent == certified => ~active /\ ~recovery
NoAuthority == ~authority
HaltNotCertified == halted => ~certified

Spec == Init /\ [][Next]_vars

=============================================================================
