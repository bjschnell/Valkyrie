# Versions and releases

Valkyrie follows [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.
Commits follow [Conventional Commits](https://www.conventionalcommits.org), and
[release-please](https://github.com/googleapis/release-please) turns them into the
next version number and the [changelog](../CHANGELOG.md).

## What a version number promises

Semver is about what users depend on. For Valkyrie that is:

- the `valk` commands, their flags and what they print where scripts read it
  (`valk upgrade --check`, `valk ls`)
- `settings.toml` keys and `VALK_*` environment variables
- the state directory: sessions, transcripts, decisions and paired devices survive
  an upgrade
- the hooks `valk setup` installs into Claude Code and Codex
- install paths and the release assets the installers and `valk upgrade` download
- the web app's pairing: a paired device stays paired across an upgrade
- the upgrade handoff: `valk upgrade` keeps every session running. A build that
  bumps `HANDOFF_VERSION` (`crates/valkyrie-daemon/src/lib.rs`) can't take over a
  running daemon, so it is a breaking change.

The wire protocol between the TUI and the daemon (`PROTOCOL`) is internal: an open
TUI follows a protocol change by itself. Key bindings, looks and wording are not
promised either, though a changed default key is worth calling out in the commit.

### Before 1.0

`0.y.z` means the surface above can still change. While the major version is 0:

| Commit | Bump | Example |
| --- | --- | --- |
| `fix:` | patch | 0.1.0 → 0.1.1 |
| `feat:` | minor | 0.1.0 → 0.2.0 |
| breaking (`feat!:` or a `BREAKING CHANGE:` footer) | minor | 0.1.0 → 0.2.0 |

From 1.0.0 on a breaking change bumps the major version. Go to 1.0 when the surface
above has settled and you are ready to promise it.

## Commit messages

```
type(scope): what changed, in the imperative

Why, if the subject doesn't say. Wrap at 72.

BREAKING CHANGE: what users have to do differently.
```

| Type | For | In the changelog | Bump |
| --- | --- | --- | --- |
| `feat` | something a user can now do | Features | minor |
| `fix` | something that was wrong for a user | Bug Fixes | patch |
| `perf` | faster or lighter, same behavior | Performance | patch |
| `revert` | undoing an earlier commit | Reverts | patch |
| `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `style` | everything else | no | none |

- **Scope** is optional and names the part: `tui`, `web`, `daemon`, `term`, `proto`,
  `agents`, `context`, `cli`, `install`, `deps`.
- **Breaking:** put `!` before the colon (`feat(cli)!: rename valk ls to valk list`)
  and say what to do in a `BREAKING CHANGE:` footer.
- Pick the type by what a user notices: a rebuilt `web/dist` that ships a fix is a
  `fix(web)`, not a `build`.
- One change per commit, so each changelog line stands alone.
- Retitle a revert `revert: <the reverted subject>`. git's default `Revert "..."` is
  let through but doesn't count toward a release.

`.githooks/commit-msg` rejects a subject that doesn't fit. Turn it on once per clone
(worktrees share it):

```sh
git config core.hooksPath .githooks
```

CI runs the same check on every commit in a pull request and on its title, which is
what a squash merge puts on `main`. A commit pushed straight to `main` is only
checked by the hook, and one that doesn't fit is left out of the changelog without
any error.

## Cutting a release

1. Commits land on `main` as usual.
2. On each push to `main`, `release-please.yml` opens or updates a release pull
   request (`chore(main): release …`). It bumps `version` in `Cargo.toml` and the
   workspace crates in `Cargo.lock`, and adds the release's section to
   `CHANGELOG.md`. Edit the changelog in that PR if a line needs rewording.
3. Merge the PR when you want the release out. release-please tags the merge commit
   `vx.y.z` and makes a draft release with the changelog as its notes.
4. `release.yml` runs the full test workflow, builds every platform, uploads the
   builds and installers, and publishes the draft. Only then do `install-release.sh`
   and `valk upgrade` see it.

A tag is never moved or reused once it is published. A fix after a release is the
next patch version.

To force a version, for example the first 1.0, add a `Release-As: 1.0.0` footer to a
commit on `main`.

The pull request is opened with `GITHUB_TOKEN`, so CI doesn't run on it. Nothing is
lost: `release.yml` tests the tagged commit on every platform before it builds, and a
red run leaves the release a draft that nobody can install.

### When a release run fails

The tag and the draft already exist, and release-please has moved on to the next
version. If the failure was flaky, re-run the failed jobs of the `release-please`
run. If the code needs a fix, abandon the version: delete the draft and its tag
(`gh release delete vX.Y.Z --cleanup-tag`), which is fine because nothing could
install it, then land the fix as a `fix:` commit and merge the next release PR.
Never delete a tag that was published.

### Cadence

Semver says what a number means, not when one comes out. The release PR collects
changes until it is merged, so the cadence is when you merge it: whenever a fix
matters, or on a fixed day such as every other Friday. Nothing in the tooling needs
to change for either.

### By hand

`git tag v0.1.1 && git push origin v0.1.1`, or run the `release` workflow from the
Actions tab with a tag, builds and publishes that tag the same way. Bump
`Cargo.toml`, `Cargo.lock` (`cargo update --workspace`) and
`.release-please-manifest.json` first, or release-please will propose that version
again.

### Setup

These are one-time settings on the GitHub repository:

- **Settings → Actions → General → Workflow permissions:** turn on **Allow GitHub
  Actions to create and approve pull requests**, so release-please can open its PR.
