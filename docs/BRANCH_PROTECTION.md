# Protect `main`

You turn these on in the GitHub website, signed in as the owner of this repository. Nothing in a pull request can flip these switches for you.

Open the repository. If you do not see a **Settings** tab, you are not signed in as the owner.

A pull request is a proposed change that waits for review. CI is the automatic check named in `.github/workflows/ci.yml`. Its two jobs are `rust` and `frontend`. A force-push rewrites history on a branch. A squash merge puts one commit on `main` for the whole pull request.

## Allow squash merges

`main` is required to stay a straight line, and each unit is squash-merged. GitHub has to be allowed to squash or the protection rule below will reject every merge.

1. Click **Settings**.
2. In the left sidebar, click **General**.
3. Scroll to **Pull Requests**.
4. Check **Allow squash merging**.
5. Uncheck **Allow merge commits**. A merge commit would bend history, and the rule in the next section blocks that.

## Require a review and a green CI run

1. Still in **Settings**, in the left sidebar, under **Code, planning, and automation**, click **Branches**. Older pages say **Code and automation**.
2. Next to **Branch protection rules**, click **Add classic branch protection rule**.
3. In **Branch name pattern**, type `main`.
4. Check **Require a pull request before merging**. Direct pushes to `main` then stop working.
5. Check **Require approvals**.
6. Set **Required number of approvals before merging** to **1**.
7. Check **Dismiss stale pull request approvals when new commits are pushed**. A new commit after approval clears that approval, and someone has to approve again.
8. Check **Require status checks to pass before merging**.
9. In the search box, add the check named `rust`, then the check named `frontend`. Those are the CI jobs. If the box has no matches, CI has not finished a run on this repository yet. Wait for one green run on `main`, come back, and add both checks.
10. Check **Require branches to be up to date before merging**, so the green run includes the latest `main`.
11. Check **Require linear history**. Pull requests then merge by squash (or rebase), not by a merge commit.
12. Leave **Allow force pushes** unchecked. That is the switch that would allow a force-push to `main`.
13. Leave **Allow deletions** unchecked.
14. Click **Create**.

## Turn on secret scanning and push protection

Secret scanning reads the repository for tokens and keys that are already committed. Push protection blocks a push that contains one.

1. In **Settings**, in the left sidebar, under **Security and quality**, click **Advanced Security**. Older pages call this **Code security and analysis** and put it under **Security**.
2. If **Secret Protection** is shown and is off, click **Enable**.
3. Next to **Push protection**, click **Enable**.
4. If a separate **Secret scanning** switch is shown and is off, click **Enable** on that too.

On a public repository these are free. On a private repository GitHub may ask for a paid Secret Protection plan before the buttons work. If a button is missing, stop. Do not paste a real token into the repo to try it.

## When it is on

- A push straight to `main` is rejected. Changes come in through a pull request.
- The pull request stays blocked until `rust` and `frontend` pass and one person has approved.
- A later commit dismisses that approval.
- History on `main` stays a straight line, one squash commit per unit.
- A force-push to `main` is rejected.
- A push that contains a recognized token is blocked.
