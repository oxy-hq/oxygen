# Verification pass

Other reviewers of this pull request produced the candidate findings below. Some of them
are wrong. Your job is to check each one against the code before anyone reads it.

For each candidate:

1. Open the code at the cited place in the checkout (the PR head), and the change with
   `git diff {{mergeBase}} {{head}} -- <file>`.
2. Try to refute it. Is the described failure real? Is it caused by this PR's changes,
   not pre-existing? Is it already handled somewhere: a caller, a guard, a type, a test,
   a constraint?
3. Decide:
   - `confirmed`: you traced it and it holds.
   - `rejected`: you found the concrete reason it is wrong, pre-existing, already
     handled, or out of scope under the rules below. Say the reason in `reason`.
   - `uncertain`: plausible, but the code did not let you settle it either way.
4. Set `severity` by the rubric below. Correct it when the candidate over- or understates
   the consequence. A request-path performance problem is never a nit.
5. When two candidates are the same root cause, confirm the clearer one and set
   `duplicate_of` on the other to its id.
6. A candidate may carry a `suggestion`: the exact new text for its lines `line` to
   `end_line`, which the author can commit in one click. Set `suggestion_ok` to true
   only when replacing exactly those lines in the head with it fixes the problem, keeps
   the code compiling and changes nothing else. Otherwise set it false: the finding is
   still posted, with its fix in words.

Reject only with a reason from the code. "Seems unlikely" is `uncertain`, not
`rejected`. Return exactly one result per candidate id.

<candidates>
{{candidates}}
</candidates>

{{rules}}

{{context}}
