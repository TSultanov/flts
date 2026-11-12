----------------------------- MODULE Ownership -----------------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANTS Books, Owners
\* `unstaged`: changes only the owner holds (a translation is never one: it is
\* staged as it is applied). `staged`: changes handed to the
\* store but not yet committed; the store lands those with or without an owner.
VARIABLES phase, live, busy, unstaged, staged, queued, disk
vars == <<phase, live, busy, unstaged, staged, queued, disk>>

Init == /\ phase = [b \in Books |-> "Cold"]
        /\ live = [b \in Books |-> {}]
        /\ busy = [b \in Books |-> FALSE]
        /\ unstaged = [b \in Books |-> FALSE]
        /\ staged = [b \in Books |-> FALSE]
        /\ queued = [b \in Books |-> 0]
        /\ disk = [b \in Books |-> TRUE]

Load(b) == /\ phase[b] = "Cold" /\ live[b] = {}
           /\ phase' = [phase EXCEPT ![b] = "Loading"]
           /\ busy' = [busy EXCEPT ![b] = TRUE]
           /\ UNCHANGED <<live, unstaged, staged, queued, disk>>
Loaded(b, o) == /\ phase[b] = "Loading" /\ live[b] = {}
                /\ phase' = [phase EXCEPT ![b] = "Active"]
                /\ live' = [live EXCEPT ![b] = @ \cup {o}]
                /\ busy' = [busy EXCEPT ![b] = FALSE]
                /\ UNCHANGED <<unstaged, staged, queued, disk>>
Accept(b) == /\ phase[b] = "Active" /\ queued[b] = 0
             /\ queued' = [queued EXCEPT ![b] = 1]
             /\ UNCHANGED <<phase, live, busy, unstaged, staged, disk>>
Apply(b) == /\ queued[b] = 1 /\ ~busy[b] /\ phase[b] \in {"Active", "Retiring"}
            /\ unstaged' = [unstaged EXCEPT ![b] = TRUE]
            /\ queued' = [queued EXCEPT ![b] = 0]
            /\ UNCHANGED <<phase, live, busy, staged, disk>>
\* The owner hands over what it holds; whether the commit then succeeds is
\* the store's concern, so staging alone lets the owner go.
Stage(b) == /\ unstaged[b] /\ ~busy[b] /\ live[b] # {}
            /\ phase[b] \in {"Active", "Retiring"}
            /\ unstaged' = [unstaged EXCEPT ![b] = FALSE]
            /\ staged' = [staged EXCEPT ![b] = TRUE]
            /\ UNCHANGED <<phase, live, busy, queued, disk>>
\* A failed commit is no step at all: the rows stay staged.
Commit(b) == /\ staged[b] /\ phase[b] # "Deleted"
             /\ staged' = [staged EXCEPT ![b] = FALSE]
             /\ disk' = [disk EXCEPT ![b] = TRUE]
             /\ UNCHANGED <<phase, live, busy, unstaged, queued>>
Retire(b) == /\ phase[b] = "Active"
             /\ phase' = [phase EXCEPT ![b] = "Retiring"]
             /\ UNCHANGED <<live, busy, unstaged, staged, queued, disk>>
Retired(b) == /\ phase[b] = "Retiring" /\ ~busy[b] /\ ~unstaged[b] /\ queued[b] = 0
              /\ phase' = [phase EXCEPT ![b] = "Cold"]
              /\ live' = [live EXCEPT ![b] = {}]
              /\ UNCHANGED <<busy, unstaged, staged, queued, disk>>
Delete(b) == /\ phase[b] \in {"Cold", "Loading", "Active", "Retiring"}
             /\ phase' = [phase EXCEPT ![b] = "Deleting"]
             /\ queued' = [queued EXCEPT ![b] = 0]
             /\ unstaged' = [unstaged EXCEPT ![b] = FALSE]
             /\ UNCHANGED <<live, busy, staged, disk>>
\* A load in flight when the delete came is let finish and then dropped.
DeletedLoad(b) == /\ phase[b] = "Deleting" /\ busy[b]
                  /\ busy' = [busy EXCEPT ![b] = FALSE]
                  /\ UNCHANGED <<phase, live, unstaged, staged, queued, disk>>
DeletedOwner(b) == /\ phase[b] = "Deleting" /\ ~busy[b]
                   /\ live' = [live EXCEPT ![b] = {}]
                   /\ UNCHANGED <<phase, busy, unstaged, staged, queued, disk>>
\* The purge outranks whatever was staged for the book.
Deleted(b) == /\ phase[b] = "Deleting" /\ ~busy[b] /\ live[b] = {}
              /\ phase' = [phase EXCEPT ![b] = "Deleted"]
              /\ disk' = [disk EXCEPT ![b] = FALSE]
              /\ staged' = [staged EXCEPT ![b] = FALSE]
              /\ UNCHANGED <<live, busy, unstaged, queued>>

Next == \E b \in Books : \/ Load(b) \/ (\E o \in Owners : Loaded(b, o))
                           \/ Accept(b) \/ Apply(b) \/ Stage(b) \/ Commit(b)
                           \/ Retire(b) \/ Retired(b) \/ Delete(b)
                           \/ DeletedLoad(b) \/ DeletedOwner(b) \/ Deleted(b)
SingleOwner == \A b \in Books : Cardinality(live[b]) <= 1
\* Nothing is lost by letting an owner go: it holds no change by then.
UnstagedPinned == \A b \in Books : unstaged[b] => live[b] # {}
DeletedNeverWrites == \A b \in Books : phase[b] = "Deleted" =>
                      /\ ~busy[b] /\ live[b] = {} /\ queued[b] = 0
                      /\ ~disk[b] /\ ~staged[b] /\ ~unstaged[b]
Spec == Init /\ [][Next]_vars
=============================================================================
