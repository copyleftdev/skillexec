------------------------------- MODULE SkillSeq -------------------------------
(***************************************************************************)
(* SEQ edges. SPEC.md §4.5 requires dst > src, and claims this buys         *)
(* acyclicity as an O(E) comparison rather than a traversal. That direction *)
(* is checked here. So is the other direction, which fails -- and the       *)
(* failure is the honest cost of the trick: a graph whose SEQ order         *)
(* disagrees with document order is acyclic but unrepresentable.            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS N, MaxEdges

Nodes == 1..N
Pairs == { <<a, b>> : a \in Nodes, b \in Nodes } \ { <<n, n>> : n \in Nodes }

VARIABLE seq
vars == <<seq>>

TypeOK == seq \in SUBSET Pairs

Succs(S)  == { e[2] : e \in { f \in seq : f[1] \in S } }

RECURSIVE Close(_, _)
Close(S, fuel) ==
    IF fuel = 0
        THEN S
        ELSE LET nxt == S \cup Succs(S)
             IN IF nxt = S THEN S ELSE Close(nxt, fuel - 1)

SeqReach(n) == Close(Succs({n}), N)
SeqAcyclic  == \A n \in Nodes : n \notin SeqReach(n)

VSeqForward == \A e \in seq : e[2] > e[1]

ForwardImpliesAcyclic == VSeqForward => SeqAcyclic
AcyclicImpliesForward == SeqAcyclic => VSeqForward

Init == /\ seq \in SUBSET Pairs
        /\ Cardinality(seq) <= MaxEdges
Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars
==============================================================================
