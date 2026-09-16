------------------------------- MODULE SkillOrg -------------------------------
(***************************************************************************)
(* Composition. SkillGraph asks whether the tree and the obligations can    *)
(* deadlock together, over one abstract obligation relation. This module    *)
(* asks a narrower question the org compiler forced into the open: when     *)
(* several skills share a file and gate each other, can the container's     *)
(* node-level cycle check see an organization that cannot ship?             *)
(*                                                                         *)
(* The shape is fixed by what `skillc org` emits. Role r occupies a subtree *)
(* rooted at node r. A gate is a Contract node INSIDE that subtree, because *)
(* SPEC.md requires a GUARDS edge to originate from a Contract; it is       *)
(* numbered R + r. "Role a gates role b" is therefore the single edge       *)
(*                                                                         *)
(*     R + a  --GUARDS-->  b                                                *)
(*                                                                         *)
(* which is the only way the format lets one role gate another.            *)
(*                                                                         *)
(* STRICT_GATES is the whole question, and it is the loader semantics that  *)
(* SPEC.md 10.7 declined to pin down. FALSE: evaluating a gate means        *)
(* reading one Contract node, and reading is not entering. TRUE: a gate is  *)
(* a role doing work, so evaluating it means entering the role that owns    *)
(* it. An organization has an execution model where a reviewer reviews, so  *)
(* TRUE is the reading that matches what the manifest says.                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS R, MaxGuards, STRICT_GATES, COMPILER_CHECKS_CYCLES, ALLOW_SELF_GATE

Roles == 1..R
\* Role r roots at node r; its gate is the Contract node R + r inside that subtree.
Gate(r) == R + r

\* A role gating itself is the ordinary precondition contract -- the commonest Contract shape in
\* the corpus. It is a separate constant because whether the strict reading can even represent it
\* is the argument that decides STRICT_GATES, and an argument that decides code should be checked.
AllPairs  == { <<a, b>> : a \in Roles, b \in Roles }
RolePairs == IF ALLOW_SELF_GATE THEN AllPairs ELSE AllPairs \ { <<n, n>> : n \in Roles }

VARIABLE guards
vars == <<guards>>

TypeOK == guards \in SUBSET RolePairs

(***************************************************************************)
(* What the container actually checks: one obligation graph over NODES,     *)
(* rejecting a cycle in it (validate.rs, SPEC.md 10.6).                     *)
(***************************************************************************)
NodeOblig == { <<Gate(e[1]), e[2]>> : e \in guards }

RECURSIVE CloseR(_, _, _)
CloseR(Rel, S, fuel) ==
    IF fuel = 0
        THEN S
        ELSE LET nxt == S \cup { e[2] : e \in { f \in Rel : f[1] \in S } }
             IN IF nxt = S THEN S ELSE CloseR(Rel, nxt, fuel - 1)

ReachR(Rel, n) == CloseR(Rel, { e[2] : e \in { f \in Rel : f[1] = n } }, 2 * R)
AcyclicR(Rel)  == \A n \in 1..(2 * R) : n \notin ReachR(Rel, n)

ContainerAccepts == AcyclicR(NodeOblig)

(***************************************************************************)
(* What an organization means. Role b cannot act until every role gating it *)
(* has acted. Under the permissive reading a gate costs nothing to satisfy, *)
(* so every role is free to act.                                           *)
(***************************************************************************)
RECURSIVE EnterCl(_, _)
EnterCl(S, fuel) ==
    IF fuel = 0
        THEN S
        ELSE LET nxt == S \cup { b \in Roles : \A a \in Roles :
                                     (<<a, b>> \in guards) => (a \in S) }
             IN IF nxt = S THEN S ELSE EnterCl(nxt, fuel - 1)

Enterable == IF STRICT_GATES THEN EnterCl({}, R) ELSE Roles

\* Every role can act. A role outside this set is one the organization can never reach.
Live == Roles \subseteq Enterable

\* The check `skillc org` performs for itself, over roles rather than nodes.
CompilerAccepts == IF COMPILER_CHECKS_CYCLES THEN AcyclicR(guards) ELSE TRUE

(***************************************************************************)
(* 1. The container's check is vacuous for organizations.                   *)
(*                                                                         *)
(* GUARDS sources are gate nodes and GUARDS targets are role roots, and     *)
(* the org compiler never makes those sets overlap. A relation whose        *)
(* endpoints are drawn from disjoint sets cannot contain a cycle, so the    *)
(* node-level check accepts EVERY organization -- including one whose       *)
(* approvals form a ring. This is expected to hold, and holding is the      *)
(* defect: the check can never fire.                                       *)
(***************************************************************************)
ContainerIsVacuous == ContainerAccepts

(***************************************************************************)
(* 2. Therefore the container cannot reject a deadlocked organization.      *)
(*                                                                         *)
(* Expected to be VIOLATED under STRICT_GATES. The counterexample is the    *)
(* circular approval chain: two roles, each gating the other.               *)
(***************************************************************************)
ContainerCatchesDeadlock == (~Live) => (~ContainerAccepts)

(***************************************************************************)
(* 3. The strict reading cannot represent an ordinary precondition contract.*)
(*                                                                         *)
(* A role whose own gate guards its own root is the commonest Contract in   *)
(* the corpus -- prerequisites, required inputs, setup. Under STRICT_GATES  *)
(* satisfying it requires entering the role that holds it, which requires   *)
(* satisfying it: not a loop a loader spins on, but a definition with no    *)
(* grounded value. Expected to be VIOLATED with ALLOW_SELF_GATE, and that   *)
(* violation is why the strict reading cannot be what the container means.  *)
(***************************************************************************)
SelfGateIsRepresentable == Live

(***************************************************************************)
(* 4. The org compiler's own check is exactly strong enough.                *)
(*                                                                         *)
(* Expected to hold with COMPILER_CHECKS_CYCLES, and to be violated without *)
(* it -- which is the canary proving this property is not vacuous.          *)
(***************************************************************************)
CompilerIsSufficient == CompilerAccepts => Live

Init == /\ guards \in SUBSET RolePairs
        /\ Cardinality(guards) <= MaxGuards

Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars
==============================================================================
