# AGENTS

## GitHub workflow (authorized)

Use the `gh` CLI for all GitHub work on this repo. Creating issues and pull
requests during development is pre-authorized — no per-action confirmation.

- Issues: `gh issue list --state all`, `gh issue view <n>`,
  `gh issue create --title "..." --body-file <file>`, `gh issue comment <n>`.
- PRs: `gh pr list`, `gh pr view <n>`, `gh pr diff <n>`, `gh pr checks`,
  `gh pr create --base develop`, `gh pr merge <n> --squash`.
- Board: `gh project list --owner <owner>`,
  `gh project item-list <number> --owner <owner> --format json`,
  `gh project field-list <number> --owner <owner>`.
  Board queries need the `read:project` scope; grant it with
  `gh auth refresh -s read:project` (or `-s project` for write access).
- Link work to issues: `Closes #<n>` in the PR body.

## Branch model (Git Flow)

- `main` — production/stable. Receives only `release/*` and `hotfix/*` merges.
- `develop` — integration branch, repository default. All feature work lands here.
- `feature/<slug>` — new work, branched from `develop`, PR base `develop`.
- `release/<version>` — stabilization cut from `develop`, PR base `main`;
  after merge, open a `main -> develop` sync PR.
- `hotfix/<slug>` — urgent production fix from `main`, PR base `main`;
  after merge, open a `main -> develop` sync PR.

Rules:

- Never push directly to `main` or `develop`; both are protected by rulesets
  (PR required, no approvals, no required checks).
- Use the prefixes above. Do not create `feat/*`, `fix/*`, or other ad-hoc names.
- Features always target `develop`; releases and hotfixes target `main`.
- After merging into `main`, always back-merge `main` into `develop`.
- Deployable work only enters `main` through `release/*` or `hotfix/*`.

