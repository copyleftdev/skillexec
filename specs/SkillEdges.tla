------------------------------ MODULE SkillEdges ------------------------------
(***************************************************************************)
(* GRAPH.md §3 claims: "everything the loader is obliged to follow stays    *)
(* inside the file and stays acyclic, so activation terminates."            *)
(*                                                                         *)
(* The validator checks SEQ forward, NEEDS acyclic, GUARDS well-typed --    *)
(* each relation on its own. This module asks whether checking them one at  *)
(* a time is the same as checking what the loader actually walks, which is  *)
(* their union.                                                             *)
(*                                                                         *)
(* Kinds are modelled as the two predicates the checks actually read. A     *)
(* six-valued kind would multiply the state space by 1296 to decide the     *)
(* same two questions.                                                      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS N, MaxEdges, CHECK_OBLIGATION_ACYCLIC

Nodes == 1..N
Pairs == { <<a, b>> : a \in Nodes, b \in Nodes } \ { <<n, n>> : n \in Nodes }

VARIABLES isContract, isResolvable, seq, needs, guards, alt
vars == <<isContract, isResolvable, seq, needs, guards, alt>>

TypeOK ==
    /\ isContract   \in [Nodes -> BOOLEAN]
    /\ isResolvable \in [Nodes -> BOOLEAN]
    /\ seq    \in SUBSET Pairs
    /\ needs  \in SUBSET Pairs
    /\ guards \in SUBSET Pairs
    /\ alt    \in SUBSET Pairs

RECURSIVE CloseR(_, _, _)
CloseR(R, S, fuel) ==
    IF fuel = 0
        THEN S
        ELSE LET nxt == S \cup { e[2] : e \in { f \in R : f[1] \in S } }
             IN IF nxt = S THEN S ELSE CloseR(R, nxt, fuel - 1)

ReachR(R, n) == CloseR(R, { e[2] : e \in { f \in R : f[1] = n } }, N)
AcyclicR(R)  == \A n \in Nodes : n \notin ReachR(R, n)

(***************************************************************************)
(* The edges a loader has no choice about following. CITES is excluded on   *)
(* purpose: it carries no load obligation and is the one kind allowed to    *)
(* cycle, which is what makes cross-skill composition expressible.          *)
(***************************************************************************)
Obligation == seq \cup needs \cup guards \cup alt

VSeqForward  == \A e \in seq : e[2] > e[1]
VGuardsTyped == \A e \in guards : isContract[e[1]]
VNeedsTarget == \A e \in needs : isResolvable[e[2]]
VNeedsAcyclic == AcyclicR(needs)
VObligAcyclic == AcyclicR(Obligation)

Validator ==
    /\ VSeqForward
    /\ VGuardsTyped
    /\ VNeedsTarget
    /\ VNeedsAcyclic
    /\ (CHECK_OBLIGATION_ACYCLIC => VObligAcyclic)

ActivationTerminates == Validator => AcyclicR(Obligation)

(***************************************************************************)
(* Each relation being individually acyclic is strictly weaker than their   *)
(* union being acyclic; this is the shape of the gap, stated on its own.    *)
(***************************************************************************)
PiecewiseIsNotEnough ==
    (AcyclicR(seq) /\ AcyclicR(needs) /\ AcyclicR(guards) /\ AcyclicR(alt))
        => AcyclicR(Obligation)

Bounded(S) == Cardinality(S) <= MaxEdges

Init ==
    /\ isContract   \in [Nodes -> BOOLEAN]
    /\ isResolvable \in [Nodes -> BOOLEAN]
    /\ seq    \in SUBSET Pairs /\ Bounded(seq)
    /\ needs  \in SUBSET Pairs /\ Bounded(needs)
    /\ guards \in SUBSET Pairs /\ Bounded(guards)
    /\ alt    \in SUBSET Pairs /\ Bounded(alt)

Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars
==============================================================================
