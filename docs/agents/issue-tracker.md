# Issue tracker

Use GitHub Issues in `pryzm-labs/retract` for issues and specs. Run `gh` from the checkout. Before creating the first issue in a session, verify the target with `gh repo view --json nameWithOwner`.

- Publish a ticket with `gh issue create --title "..." --body-file <file>`.
- Fetch a ticket with `gh issue view <number> --json number,title,body,labels,comments`.
- List open tickets with `gh issue list --state open`. Use `--label` to filter by triage role.
- Apply or remove a label with `gh issue edit <number> --add-label` or `--remove-label`, followed by the label name.
- Link child tickets as GitHub sub-issues and record blockers as GitHub issue dependencies. If those features are unavailable, use `Part of #<parent>` or `Blocked by: #<blocker>` in the child issue. A ticket is ready to start once every blocker is closed.
- To close a ticket, post the outcome with `gh issue comment` and then run `gh issue close`.

For `/wayfinder`, use one `wayfinder:map` issue with child tickets in map order. Label each child by type. Claim the first open, unassigned child with no open blockers. When it is resolved, record the result on the child and link it from the map.

PRs as a request surface: no.
