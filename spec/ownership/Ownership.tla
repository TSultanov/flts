----------------------------- MODULE Ownership -----------------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANTS Books, Owners
VARIABLES phase, live, busy, dirty, failed, queued, disk, generation, issued, committed
vars == <<phase, live, busy, dirty, failed, queued, disk, generation, issued, committed>>

Init == /\ phase = [b \in Books |-> "Cold"]
        /\ live = [b \in Books |-> {}]
        /\ busy = [b \in Books |-> FALSE]
        /\ dirty = [b \in Books |-> FALSE]
        /\ failed = [b \in Books |-> FALSE]
        /\ queued = [b \in Books |-> 0]
        /\ disk = [b \in Books |-> TRUE]
        /\ generation = [b \in Books |-> 0]
        /\ issued = [b \in Books |-> 0]
        /\ committed = [b \in Books |-> 0]

Load(b) == /\ phase[b] = "Cold" /\ live[b] = {} /\ generation[b] < 2
           /\ phase' = [phase EXCEPT ![b] = "Loading"]
           /\ busy' = [busy EXCEPT ![b] = TRUE]
           /\ generation' = [generation EXCEPT ![b] = @ + 1]
           /\ committed' = [committed EXCEPT ![b] = 0]
           /\ UNCHANGED <<live, dirty, failed, queued, disk, issued>>
Loaded(b, o) == /\ phase[b] = "Loading" /\ live[b] = {}
                /\ phase' = [phase EXCEPT ![b] = "Active"]
                /\ live' = [live EXCEPT ![b] = @ \cup {o}]
                /\ busy' = [busy EXCEPT ![b] = FALSE]
                /\ UNCHANGED <<dirty, failed, queued, disk, generation, issued, committed>>
Accept(b) == /\ phase[b] = "Active" /\ queued[b] = 0 /\ issued[b] = generation[b]
             /\ queued' = [queued EXCEPT ![b] = 1]
             /\ UNCHANGED <<phase, live, busy, dirty, failed, disk, generation, issued, committed>>
Apply(b) == /\ queued[b] = 1 /\ ~busy[b] /\ phase[b] \in {"Active", "Retiring"}
            /\ issued[b] = generation[b]
            /\ committed' = [committed EXCEPT ![b] = issued[b]]
            /\ dirty' = [dirty EXCEPT ![b] = TRUE]
            /\ queued' = [queued EXCEPT ![b] = 0]
            /\ UNCHANGED <<phase, live, busy, failed, disk, generation, issued>>
Save(b) == /\ dirty[b] /\ ~busy[b] /\ live[b] # {}
           /\ busy' = [busy EXCEPT ![b] = TRUE]
           /\ UNCHANGED <<phase, live, dirty, failed, queued, disk, generation, issued, committed>>
Saved(b) == /\ busy[b] /\ phase[b] # "Loading"
            /\ busy' = [busy EXCEPT ![b] = FALSE]
            /\ dirty' = [dirty EXCEPT ![b] = FALSE]
            /\ failed' = [failed EXCEPT ![b] = FALSE]
            /\ disk' = [disk EXCEPT ![b] = TRUE]
            /\ UNCHANGED <<phase, live, queued, generation, issued, committed>>
SaveFailed(b) == /\ busy[b] /\ phase[b] \in {"Active", "Retiring"}
                 /\ busy' = [busy EXCEPT ![b] = FALSE]
                 /\ dirty' = [dirty EXCEPT ![b] = TRUE]
                 /\ failed' = [failed EXCEPT ![b] = TRUE]
                 /\ UNCHANGED <<phase, live, queued, disk, generation, issued, committed>>
Retire(b) == /\ phase[b] = "Active"
             /\ phase' = [phase EXCEPT ![b] = "Retiring"]
             /\ UNCHANGED <<live, busy, dirty, failed, queued, disk, generation, issued, committed>>
Retired(b) == /\ phase[b] = "Retiring" /\ ~busy[b] /\ ~dirty[b] /\ queued[b] = 0
              /\ phase' = [phase EXCEPT ![b] = "Cold"]
              /\ live' = [live EXCEPT ![b] = {}]
              /\ UNCHANGED <<busy, dirty, failed, queued, disk, generation, issued, committed>>
RetirementFailed(b) == /\ phase[b] = "Retiring" /\ failed[b] /\ ~busy[b]
                      /\ phase' = [phase EXCEPT ![b] = "Active"]
                      /\ UNCHANGED <<live, busy, dirty, failed, queued, disk, generation, issued, committed>>
Delete(b) == /\ phase[b] \in {"Cold", "Loading", "Active", "Retiring"}
             /\ phase' = [phase EXCEPT ![b] = "Deleting"]
             /\ queued' = [queued EXCEPT ![b] = 0]
             /\ dirty' = [dirty EXCEPT ![b] = FALSE]
             /\ UNCHANGED <<live, busy, failed, disk, generation, issued, committed>>
DeletedOwner(b) == /\ phase[b] = "Deleting" /\ ~busy[b]
                   /\ live' = [live EXCEPT ![b] = {}]
                   /\ UNCHANGED <<phase, busy, dirty, failed, queued, disk, generation, issued, committed>>
Deleted(b) == /\ phase[b] = "Deleting" /\ ~busy[b] /\ live[b] = {}
              /\ phase' = [phase EXCEPT ![b] = "Deleted"]
              /\ disk' = [disk EXCEPT ![b] = FALSE]
              /\ UNCHANGED <<live, busy, dirty, failed, queued, generation, issued, committed>>

Capture(b) == /\ phase[b] = "Active" /\ queued[b] = 0
              /\ issued' = [issued EXCEPT ![b] = generation[b]]
              /\ UNCHANGED <<phase, live, busy, dirty, failed, queued, disk, generation, committed>>
GenerationChecked == \A b \in Books : committed[b] \in {0, generation[b]}

Next == \E b \in Books : \/ Load(b) \/ (\E o \in Owners : Loaded(b, o))
                           \/ Capture(b) \/ Accept(b) \/ Apply(b) \/ Save(b) \/ Saved(b)
                           \/ SaveFailed(b) \/ Retire(b) \/ Retired(b)
                           \/ RetirementFailed(b) \/ Delete(b)
                           \/ DeletedOwner(b) \/ Deleted(b)
SingleOwner == \A b \in Books : Cardinality(live[b]) <= 1
DirtyPinned == \A b \in Books : dirty[b] => live[b] # {}
DeletedNeverWrites == \A b \in Books : phase[b] = "Deleted" =>
                      /\ ~busy[b] /\ live[b] = {} /\ queued[b] = 0 /\ ~disk[b]
Spec == Init /\ [][Next]_vars
=============================================================================
