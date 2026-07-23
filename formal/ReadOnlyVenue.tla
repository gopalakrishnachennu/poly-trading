------------------------- MODULE ReadOnlyVenue -------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS MaxEpoch, MaxParameterVersion

Channels == {"public", "user", "metadata", "reference"}
Scenarios == {"public_sync", "user_sync", "parameters", "normal", "restart",
              "post_only", "cancel_only", "rate", "failure", "reconnect"}

VARIABLES registered, epoch, readyChannels, parameterVersion, mode,
          recovery, reason, covered, certified, mutation, authority, halted

vars == <<registered, epoch, readyChannels, parameterVersion, mode,
          recovery, reason, covered, certified, mutation, authority, halted>>

Init == /\ registered = FALSE /\ epoch = 0 /\ readyChannels = {}
        /\ parameterVersion = 0 /\ mode = "none" /\ recovery = FALSE
        /\ reason = "none" /\ covered = {} /\ certified = FALSE
        /\ mutation = FALSE /\ authority = FALSE /\ halted = FALSE

Register == /\ ~registered /\ ~halted /\ registered' = TRUE
            /\ UNCHANGED <<epoch, readyChannels, parameterVersion, mode,
                           recovery, reason, covered, certified, mutation,
                           authority, halted>>

Sync(c) == /\ registered /\ ~recovery /\ ~halted /\ c \in Channels
              /\ epoch > 0 /\ readyChannels' = readyChannels \cup {c}
           /\ covered' = IF c = "public" THEN covered \cup {"public_sync"}
                          ELSE IF c = "user" THEN covered \cup {"user_sync"}
                          ELSE covered
           /\ UNCHANGED <<registered, epoch, parameterVersion, mode, recovery,
                          reason, certified, mutation, authority, halted>>

StartEpoch == /\ registered /\ epoch = 0 /\ ~recovery /\ ~halted
              /\ epoch' = 1
              /\ UNCHANGED <<registered, readyChannels, parameterVersion, mode,
                             recovery, reason, covered, certified, mutation,
                             authority, halted>>

Parameters == /\ registered /\ ~recovery /\ ~halted
              /\ parameterVersion < MaxParameterVersion
              /\ parameterVersion' = parameterVersion + 1
              /\ covered' = covered \cup {"parameters"}
              /\ UNCHANGED <<registered, epoch, readyChannels, mode, recovery,
                             reason, certified, mutation, authority, halted>>

SetMode(m) == /\ m \in {"normal", "post_only", "cancel_only"}
              /\ registered /\ ~recovery /\ ~halted
              /\ mode' = m
              /\ covered' = covered \cup
                   {IF m = "normal" THEN "normal"
                    ELSE IF m = "post_only" THEN "post_only" ELSE "cancel_only"}
              /\ UNCHANGED <<registered, epoch, readyChannels, parameterVersion,
                             recovery, reason, certified, mutation, authority, halted>>

RateLimit == /\ registered /\ ~recovery /\ ~halted
             /\ covered' = covered \cup {"rate"}
             /\ UNCHANGED <<registered, epoch, readyChannels, parameterVersion,
                            mode, recovery, reason, certified, mutation,
                            authority, halted>>

Disrupt(r) == /\ r \in {"restart", "failure"}
              /\ registered /\ ~recovery /\ ~halted /\ epoch > 0 /\ epoch < MaxEpoch
              /\ recovery' = TRUE /\ reason' = r
              /\ readyChannels' = {} /\ parameterVersion' = 0
              /\ mode' = "recovering"
              /\ covered' = IF r = "failure" THEN covered \cup {"failure"} ELSE covered
              /\ UNCHANGED <<registered, epoch, certified, mutation, authority, halted>>

Recover == /\ recovery /\ ~halted /\ epoch < MaxEpoch
           /\ epoch' = epoch + 1 /\ readyChannels' = Channels
           /\ parameterVersion' = 1 /\ mode' = "normal"
           /\ recovery' = FALSE /\ reason' = "none"
           /\ covered' = covered \cup {"reconnect"} \cup
                (IF reason = "restart" THEN {"restart"} ELSE {})
           /\ UNCHANGED <<registered, certified, mutation, authority, halted>>

Finalize == /\ ~recovery /\ readyChannels = Channels
            /\ parameterVersion > 0 /\ mode \in {"normal", "post_only", "cancel_only"}
            /\ Scenarios \subseteq covered /\ ~certified /\ ~halted
            /\ certified' = TRUE
            /\ UNCHANGED <<registered, epoch, readyChannels, parameterVersion,
                           mode, recovery, reason, covered, mutation, authority, halted>>

Fail == /\ ~halted /\ ~certified /\ halted' = TRUE
        /\ UNCHANGED <<registered, epoch, readyChannels, parameterVersion, mode,
                       recovery, reason, covered, certified, mutation, authority>>
TerminalStutter == (halted \/ certified) /\ UNCHANGED vars

Next == Register \/ StartEpoch \/ (\E c \in Channels : Sync(c)) \/ Parameters
        \/ (\E m \in {"normal", "post_only", "cancel_only"} : SetMode(m))
        \/ RateLimit \/ (\E r \in {"restart", "failure"} : Disrupt(r))
        \/ Recover \/ Finalize \/ Fail \/ TerminalStutter

TypeInvariant == /\ registered \in BOOLEAN /\ epoch \in 0..MaxEpoch
                 /\ readyChannels \subseteq Channels /\ parameterVersion \in 0..MaxParameterVersion
                 /\ mode \in {"none", "normal", "post_only", "cancel_only", "recovering"}
                 /\ recovery \in BOOLEAN /\ reason \in {"none", "restart", "failure"}
                 /\ covered \subseteq Scenarios /\ certified \in BOOLEAN
                 /\ mutation \in BOOLEAN /\ authority \in BOOLEAN /\ halted \in BOOLEAN
RecoveryInvalidates == recovery => readyChannels = {} /\ parameterVersion = 0 /\ mode = "recovering"
ReadyIsComplete == certified => readyChannels = Channels /\ ~recovery /\ Scenarios \subseteq covered
NoMutation == ~mutation
NoAuthority == ~authority
HaltNotCertified == halted => ~certified

Spec == Init /\ [][Next]_vars
========================================================================
