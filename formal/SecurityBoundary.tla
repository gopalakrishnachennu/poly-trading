----------------------- MODULE SecurityBoundary -----------------------
EXTENDS Integers, FiniteSets, TLC

Providers == {"vault", "kms", "hsm"}

VARIABLES registered, progress, epoch, active, revoked, recovery,
          providers, certified, secret, signature, authority, halted

vars == <<registered, progress, epoch, active, revoked, recovery,
          providers, certified, secret, signature, authority, halted>>

Init ==
  /\ registered = FALSE /\ progress = 0 /\ epoch = 0
  /\ active = FALSE /\ revoked = {} /\ recovery = FALSE
  /\ providers = {} /\ certified = FALSE
  /\ secret = FALSE /\ signature = FALSE /\ authority = FALSE
  /\ halted = FALSE

Register ==
  /\ ~registered /\ ~halted
  /\ registered' = TRUE
  /\ UNCHANGED <<progress, epoch, active, revoked, recovery, providers,
                 certified, secret, signature, authority, halted>>

Issue(p) ==
  /\ registered /\ progress = 0 /\ ~active /\ ~recovery /\ ~halted
  /\ p \in Providers
  /\ progress' = 1 /\ epoch' = 1 /\ active' = TRUE
  /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, revoked, recovery, certified,
                 secret, signature, authority, halted>>

Rotate(p) ==
  /\ progress = 1 /\ active /\ ~recovery /\ ~halted /\ p \in Providers
  /\ progress' = 2 /\ epoch' = 2 /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, active, revoked, recovery, certified,
                 secret, signature, authority, halted>>

SafeStep(p) ==
  /\ progress \in 2..6 /\ active /\ ~recovery /\ ~halted
  /\ p \in Providers
  /\ progress' = progress + 1 /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery, certified,
                 secret, signature, authority, halted>>

Compromise(p) ==
  /\ progress = 7 /\ active /\ ~recovery /\ ~halted /\ p \in Providers
  /\ progress' = 8 /\ active' = FALSE
  /\ revoked' = revoked \cup {epoch} /\ recovery' = TRUE
  /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, epoch, certified, secret, signature, authority, halted>>

Recover ==
  /\ progress = 8 /\ ~active /\ recovery /\ ~halted
  /\ recovery' = FALSE
  /\ UNCHANGED <<registered, progress, epoch, active, revoked, providers,
                 certified, secret, signature, authority, halted>>

Disaster(p) ==
  /\ progress = 8 /\ ~active /\ ~recovery /\ ~halted /\ p \in Providers
  /\ progress' = 9 /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, epoch, active, revoked, recovery, certified,
                 secret, signature, authority, halted>>

Reissue ==
  /\ progress = 9 /\ epoch = 2 /\ ~active /\ ~recovery /\ ~halted
  /\ epoch' = 3 /\ active' = TRUE
  /\ UNCHANGED <<registered, progress, revoked, recovery, providers,
                 certified, secret, signature, authority, halted>>

Revoke(p) ==
  /\ progress = 9 /\ epoch = 3 /\ active /\ ~recovery /\ ~halted
  /\ p \in Providers
  /\ progress' = 10 /\ active' = FALSE
  /\ revoked' = revoked \cup {epoch} /\ providers' = providers \cup {p}
  /\ UNCHANGED <<registered, epoch, recovery, certified,
                 secret, signature, authority, halted>>

Finalize ==
  /\ progress = 10 /\ ~active /\ ~recovery /\ Providers \subseteq providers
  /\ ~certified /\ ~halted
  /\ certified' = TRUE
  /\ UNCHANGED <<registered, progress, epoch, active, revoked, recovery,
                 providers, secret, signature, authority, halted>>

Fail == ~halted /\ ~certified /\ halted' = TRUE /\ UNCHANGED <<registered, progress, epoch, active, revoked, recovery, providers, certified, secret, signature, authority>>
TerminalStutter == (halted \/ certified) /\ UNCHANGED vars

Next == Register \/ (\E p \in Providers : Issue(p) \/ Rotate(p) \/ SafeStep(p) \/ Compromise(p) \/ Disaster(p) \/ Revoke(p)) \/ Recover \/ Reissue \/ Finalize \/ Fail \/ TerminalStutter

TypeInvariant == /\ registered \in BOOLEAN /\ progress \in 0..10 /\ epoch \in 0..3 /\ active \in BOOLEAN /\ revoked \subseteq 0..3 /\ recovery \in BOOLEAN /\ providers \subseteq Providers /\ certified \in BOOLEAN /\ secret \in BOOLEAN /\ signature \in BOOLEAN /\ authority \in BOOLEAN /\ halted \in BOOLEAN
NoActiveRecovery == ~(active /\ recovery)
RevokedInactive == active => ~(epoch \in revoked)
RecoveryHasRevocation == recovery => Cardinality(revoked) > 0
CertifiedComplete == certified => progress = 10 /\ Providers \subseteq providers /\ ~active /\ ~recovery
NoSecret == ~secret
NoSignature == ~signature
NoAuthority == ~authority
HaltNotCertified == halted => ~certified

Spec == Init /\ [][Next]_vars
=======================================================================
