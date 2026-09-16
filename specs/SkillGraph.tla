------------------------------ MODULE SkillGraph ------------------------------
(***************************************************************************)
(* The join. SkillTree and SkillEdges share no variables, so each can pass  *)
(* while the pair is wrong: the loader descends the CONTAINS tree AND       *)
(* follows obligation edges, and the interaction between them is checked    *)
(* nowhere else.                                                            *)
(*                                                                         *)
(* Obligation is abstracted to one relation here. Which edge kind an        *)
(* obligation came from is SkillEdges' question; whether the tree and the   *)
(* obligations can deadlock together is this module's.                      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS N, MaxTier, MaxEdges, INCLUDE_DESCENT

Nodes == 1..N
NONE  == 0
Pairs == { <<a, b>> : a \in Nodes, b \in Nodes } \ { <<n, n>> : n \in Nodes }

VARIABLES parent, tier, oblig
vars == <<parent, tier, oblig>>

TypeOK == /\ parent \in [Nodes -> 0..N]
          /\ tier   \in [Nodes -> 0..MaxTier]
          /\ oblig  \in SUBSET Pairs

RECURSIVE Up(_, _)
Up(n, fuel) ==
    IF (fuel = 0) \/ (parent[n] = NONE)
        THEN {}
        ELSE {parent[n]} \cup Up(parent[n], fuel - 1)

Ancestors(n)   == Up(n, N)
Descendants(n) == { m \in Nodes : n \in Ancestors(m) }
Subtree(n)     == {n} \cup Descendants(n)
Contiguous(S)  == \A a \in S : \A b \in S : \A c \in Nodes :
                      ((a <= c) /\ (c <= b)) => (c \in S)

\* Descent is how a loader walks into a subtree: parent to child.
Descent == { <<parent[n], n>> : n \in { m \in Nodes : parent[m] \in Nodes } }
Walked  == IF INCLUDE_DESCENT THEN oblig \cup Descent ELSE oblig

RECURSIVE CloseR(_, _, _)
CloseR(R, S, fuel) ==
    IF fuel = 0
        THEN S
        ELSE LET nxt == S \cup { e[2] : e \in { f \in R : f[1] \in S } }
             IN IF nxt = S THEN S ELSE CloseR(R, nxt, fuel - 1)

ReachR(R, n) == CloseR(R, { e[2] : e \in { f \in R : f[1] = n } }, N)
AcyclicR(R)  == \A n \in Nodes : n \notin ReachR(R, n)

TreeSemantic ==
    /\ parent[1] = NONE
    /\ \A n \in Nodes \ {1} : parent[n] # NONE
    /\ \A n \in Nodes : n \notin Ancestors(n)
    /\ \A n \in Nodes \ {1} : 1 \in Ancestors(n)
    /\ \A n \in Nodes : \A m \in Descendants(n) : n < m
    /\ \A n \in Nodes : Contiguous(Subtree(n))
    /\ \A n \in Nodes \ {1} : (parent[n] \in Nodes) => (tier[n] >= tier[parent[n]])
    /\ tier[1] = 0

TreeValidator ==
    /\ parent[1] = NONE
    /\ \A n \in Nodes \ {1} : parent[n] # NONE
    /\ \A n \in Nodes \ {1} : parent[n] \in ({n - 1} \cup Ancestors(n - 1))
    /\ \A n \in Nodes \ {1} : (parent[n] \in Nodes) => (tier[n] >= tier[parent[n]])
    /\ tier[1] = 0

Validator == TreeValidator /\ AcyclicR(oblig)

(***************************************************************************)
(* The composed claim: a file this implementation accepts describes a tree  *)
(* the model recognises, and a walk that terminates.                        *)
(***************************************************************************)
Composed == Validator => (TreeSemantic /\ AcyclicR(Walked))

Init == /\ parent \in [Nodes -> 0..N]
        /\ tier   \in [Nodes -> 0..MaxTier]
        /\ oblig  \in SUBSET Pairs
        /\ Cardinality(oblig) <= MaxEdges

Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars
==============================================================================
