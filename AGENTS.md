# AGENTS

## GitHub workflow (authorized)

Use the `gh` CLI for all GitHub work on this repo. Creating issues and pull
requests during development is pre-authorized — no per-action confirmation.

- Issues: `gh issue list --state all`, `gh issue view <n>`,
  `gh issue create --title "..." --body-file <file>`, `gh issue comment <n>`.
- PRs: `gh pr list`, `gh pr view <n>`, `gh pr diff <n>`, `gh pr checks`,
  `gh pr create --fill`, `gh pr merge <n> --squash`.
- Board: `gh project list --owner <owner>`,
  `gh project item-list <number> --owner <owner> --format json`,
  `gh project field-list <number> --owner <owner>`.
- Link work to issues: `Closes #<n>` in the PR body.

Board queries need the `read:project` scope; grant it with
`gh auth refresh -s project` (read+write) or `-s read:project` (read-only).
