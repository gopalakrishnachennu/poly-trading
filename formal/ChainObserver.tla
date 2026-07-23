-------------------------- MODULE ChainObserver --------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS MaxHead, MaxFinalized

Providers == {"primary", "secondary", "archive"}
Scenarios == {"agreement", "head", "finality", "reorg", "disagreement",
              "stale", "chain_mismatch", "balance", "allowance", "transaction"}

VARIABLES registered, observedProviders, head, finalized, finalizedHash,
          collateral, allowance, txFinalized, recovery, covered, certified,
          rpcMutation, walletMutation, authority, halted

vars == <<registered, observedProviders, head, finalized, finalizedHash,
          collateral, allowance, txFinalized, recovery, covered, certified,
          rpcMutation, walletMutation, authority, halted>>

Init == /\ registered = FALSE /\ observedProviders = {} /\ head = 0
        /\ finalized = 0 /\ finalizedHash = 0 /\ collateral = 0
        /\ allowance = 0 /\ txFinalized = FALSE /\ recovery = FALSE
        /\ covered = {} /\ certified = FALSE /\ rpcMutation = FALSE
        /\ walletMutation = FALSE /\ authority = FALSE /\ halted = FALSE

Register == /\ ~registered /\ ~halted /\ registered' = TRUE
            /\ UNCHANGED <<observedProviders, head, finalized, finalizedHash,
                           collateral, allowance, txFinalized, recovery, covered,
                           certified, rpcMutation, walletMutation, authority, halted>>

Agree == /\ registered /\ ~recovery /\ ~certified /\ ~halted /\ head < MaxHead
         /\ observedProviders' = Providers /\ head' = head + 1
         /\ finalized' = IF finalized < MaxFinalized THEN finalized + 1 ELSE finalized
         /\ finalizedHash' = IF finalized' > finalized THEN finalized' ELSE finalizedHash
         /\ collateral' = collateral + 1 /\ allowance' = allowance + 1
         /\ txFinalized' = TRUE
         /\ covered' = covered \cup {"agreement", "head", "balance", "allowance", "transaction"}
                        \cup (IF finalized' > finalized THEN {"finality"} ELSE {})
         /\ UNCHANGED <<registered, recovery, certified, rpcMutation,
                        walletMutation, authority, halted>>

RecordFailure(s) == /\ s \in {"disagreement", "stale", "chain_mismatch"}
                    /\ registered /\ ~halted /\ ~certified
                    /\ covered' = covered \cup {s}
                    /\ UNCHANGED <<registered, observedProviders, head, finalized,
                                   finalizedHash, collateral, allowance, txFinalized,
                                   recovery, certified, rpcMutation, walletMutation,
                                   authority, halted>>

Reorg == /\ registered /\ observedProviders = Providers /\ head > finalized
         /\ ~recovery /\ ~certified /\ ~halted /\ recovery' = TRUE
         /\ observedProviders' = {} /\ covered' = covered \cup {"reorg"}
         /\ UNCHANGED <<registered, head, finalized, finalizedHash, collateral,
                        allowance, txFinalized, certified, rpcMutation,
                        walletMutation, authority, halted>>

Recover == /\ recovery /\ ~certified /\ ~halted /\ recovery' = FALSE
           /\ observedProviders' = Providers
           /\ UNCHANGED <<registered, head, finalized, finalizedHash, collateral,
                          allowance, txFinalized, covered, certified, rpcMutation,
                          walletMutation, authority, halted>>

Finalize == /\ registered /\ ~recovery /\ observedProviders = Providers
            /\ Scenarios \subseteq covered /\ ~certified /\ ~halted
            /\ certified' = TRUE
            /\ UNCHANGED <<registered, observedProviders, head, finalized,
                           finalizedHash, collateral, allowance, txFinalized,
                           recovery, covered, rpcMutation, walletMutation,
                           authority, halted>>

Fail == /\ ~halted /\ ~certified /\ halted' = TRUE
        /\ UNCHANGED <<registered, observedProviders, head, finalized,
                       finalizedHash, collateral, allowance, txFinalized,
                       recovery, covered, certified, rpcMutation, walletMutation,
                       authority>>

TerminalStutter == (halted \/ certified) /\ UNCHANGED vars

Next == Register \/ Agree
        \/ (\E s \in {"disagreement", "stale", "chain_mismatch"} : RecordFailure(s))
        \/ Reorg \/ Recover \/ Finalize \/ Fail \/ TerminalStutter

TypeInvariant == /\ registered \in BOOLEAN /\ observedProviders \subseteq Providers
                 /\ head \in 0..MaxHead /\ finalized \in 0..MaxFinalized
                 /\ finalizedHash \in 0..MaxFinalized /\ collateral \in 0..MaxHead
                 /\ allowance \in 0..MaxHead /\ txFinalized \in BOOLEAN
                 /\ recovery \in BOOLEAN /\ covered \subseteq Scenarios
                 /\ certified \in BOOLEAN /\ rpcMutation \in BOOLEAN
                 /\ walletMutation \in BOOLEAN /\ authority \in BOOLEAN
                 /\ halted \in BOOLEAN
FinalityMonotonic == finalized <= head
ReorgInvalidates == recovery => observedProviders = {}
CertifiedHasAgreement == certified => observedProviders = Providers /\ ~recovery /\ Scenarios \subseteq covered
NoRpcMutation == ~rpcMutation
NoWalletMutation == ~walletMutation
NoAuthority == ~authority
HaltNotCertified == halted => ~certified

Spec == Init /\ [][Next]_vars
=============================================================================
