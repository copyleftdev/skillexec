------------------------------ MODULE SkillTree ------------------------------
(***************************************************************************)
(* The CONTAINS tree of a .skill file, and the question the implementation  *)
(* actually rests on: is the one forward pass in validate.rs equivalent to  *)
(* the semantic invariants in GRAPH.md §9, or merely close to them?         *)
(*                                                                         *)
(* validate.rs never builds a child list, never detects a cycle and never   *)
(* allocates. It relies on nodes being stored in pre-order, which turns     *)
(* "the parent pointers form a single rooted tree whose subtrees are        *)
(* contiguous" into two comparisons per node. That is the claim under test. *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS N,                    \* nodes are 1..N; node 1 is the root
          MaxTier,
          CHECK_PARENT_BEFORE,  \* canaries disable exactly one check each
          CHECK_PREORDER_SCAN,
          CHECK_TIER,
          CHECK_ROOT_TIER

Nodes == 1..N
NONE  == 0

VARIABLES parent, tier
vars == <<parent, tier>>

TypeOK == /\ parent \in [Nodes -> 0..N]
          /\ tier   \in [Nodes -> 0..MaxTier]

(***************************************************************************)
(* Fuel-bounded so that a malformed file describing a parent cycle makes    *)
(* this terminate rather than hang. A model that only terminates on valid   *)
(* input cannot be used to judge invalid input.                             *)
(***************************************************************************)
RECURSIVE Up(_, _)
Up(n, fuel) ==
    IF (fuel = 0) \/ (parent[n] = NONE)
        THEN {}
        ELSE {parent[n]} \cup Up(parent[n], fuel - 1)

Ancestors(n)   == Up(n, N)
Descendants(n) == { m \in Nodes : n \in Ancestors(m) }
Subtree(n)     == {n} \cup Descendants(n)

Contiguous(S) ==
    \A a \in S : \A b \in S : \A c \in Nodes : ((a <= c) /\ (c <= b)) => (c \in S)

(***************************************************************************)
(* What the format requires, stated without reference to how it is checked. *)
(***************************************************************************)
IsRootedTree ==
    /\ parent[1] = NONE
    /\ \A n \in Nodes \ {1} : parent[n] # NONE
    /\ \A n \in Nodes : n \notin Ancestors(n)
    /\ \A n \in Nodes \ {1} : 1 \in Ancestors(n)

IsPreOrderLayout ==
    /\ \A n \in Nodes : \A m \in Descendants(n) : n < m
    /\ \A n \in Nodes : Contiguous(Subtree(n))

TierMonotone ==
    \A n \in Nodes \ {1} : (parent[n] \in Nodes) => (tier[n] >= tier[parent[n]])

RootIsRouting == tier[1] = 0

Semantic == IsRootedTree /\ IsPreOrderLayout /\ TierMonotone /\ RootIsRouting

(***************************************************************************)
(* What validate.rs computes, in the order it computes it.                  *)
(***************************************************************************)
VRootHasNoParent  == parent[1] = NONE
VNonRootHasParent == \A n \in Nodes \ {1} : parent[n] # NONE
VParentBefore     == \A n \in Nodes \ {1} : parent[n] < n
VPreOrderScan     == \A n \in Nodes \ {1} : parent[n] \in ({n - 1} \cup Ancestors(n - 1))
VTierMonotone     == \A n \in Nodes \ {1} : (parent[n] \in Nodes) => (tier[n] >= tier[parent[n]])
VRootTier         == tier[1] = 0

Validator ==
    /\ VRootHasNoParent
    /\ VNonRootHasParent
    /\ (CHECK_PARENT_BEFORE  => VParentBefore)
    /\ (CHECK_PREORDER_SCAN  => VPreOrderScan)
    /\ (CHECK_TIER           => VTierMonotone)
    /\ (CHECK_ROOT_TIER      => VRootTier)

(***************************************************************************)
(* Soundness is the property that matters: the validator must never accept  *)
(* a file that violates the model. Completeness matters too, because a      *)
(* validator that rejects legal files is a format nobody can write.         *)
(***************************************************************************)
(***************************************************************************)
(* TLC found this while a canary was trying to prove the opposite: the      *)
(* pre-order scan already implies `parent < self`, because Ancestors(n-1)   *)
(* can only contain nodes below n once every earlier parent is below its    *)
(* own node. So SPEC.md's claim that one comparison per node proves the     *)
(* tree invariant is too strong -- the comparison gives acyclicity and      *)
(* rootedness, and contiguity needs the ancestor walk.                      *)
(***************************************************************************)
ParentBeforeIsRedundant ==
    (VRootHasNoParent /\ VNonRootHasParent /\ VPreOrderScan) => VParentBefore

(***************************************************************************)
(* And the converse fails, which is the half that matters: `parent < self`  *)
(* alone permits a forest of interleaved subtrees.                          *)
(***************************************************************************)
ParentBeforeIsNotEnough ==
    (VRootHasNoParent /\ VNonRootHasParent /\ VParentBefore) => IsPreOrderLayout

Soundness    == Validator => Semantic
Completeness == Semantic  => Validator
Equivalence  == Validator <=> Semantic

Init == /\ parent \in [Nodes -> 0..N]
        /\ tier   \in [Nodes -> 0..MaxTier]

Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars
==============================================================================
