# Changelog

Releases from 0.1.0 on follow [Semantic Versioning](https://semver.org) and are
written by release-please from [Conventional Commits](docs/releasing.md).

## [0.5.0](https://github.com/bjschnell/Valkyrie/compare/v0.4.0...v0.5.0) (2026-10-30)


### Features

* **backlog:** agents file, take and track their own work ([af3252b](https://github.com/bjschnell/Valkyrie/commit/af3252b5a8fff933618ee576152daad6db095782))
* **backlog:** one session per item, and an open PR shows in review ([0b58a03](https://github.com/bjschnell/Valkyrie/commit/0b58a03f2e99d40edff1798949dfb0a9cccc0307))
* **backlog:** one session per item, PR open shows review, agents track their own work ([9e0b350](https://github.com/bjschnell/Valkyrie/commit/9e0b3504e5d7889c96e84010efe4e26009511b54))
* **backlog:** take an item from a session, ship every merged one at once ([1074773](https://github.com/bjschnell/Valkyrie/commit/1074773055be54d20330a5f9f785ff892a81308f))
* **backlog:** take an item from a session, ship every merged one at once ([6eb328e](https://github.com/bjschnell/Valkyrie/commit/6eb328e25c8ec4a88ea6688795dccca1cc824608))
* **tui:** Alt-a captures a backlog item from anywhere ([b43f946](https://github.com/bjschnell/Valkyrie/commit/b43f946499904a8e48d9c92f217b54979510cffc))
* **tui:** Alt-a captures a backlog item from anywhere ([5e2e393](https://github.com/bjschnell/Valkyrie/commit/5e2e39381df807d88dfbc6bb1a6afd81c071d5a4))


### Bug Fixes

* **backlog:** a take never locks the user out of an item ([a232f5d](https://github.com/bjschnell/Valkyrie/commit/a232f5d2e410e09ab238c4c02e772b41f9691293))
* **backlog:** after a web clash, move on only the clashing fields ([e39da1f](https://github.com/bjschnell/Valkyrie/commit/e39da1f89c2967e8b041f3aa549c92df1605363e))
* **backlog:** an edit changes only the fields it gives ([cfd77f5](https://github.com/bjschnell/Valkyrie/commit/cfd77f5c2c0c21c5a1bf6f9cf8036d6fbbb09bb3))
* **backlog:** an edit changes only the fields it gives ([03ddf1e](https://github.com/bjschnell/Valkyrie/commit/03ddf1ef05c1de7a09e756d94184f2b53ca268eb))
* **backlog:** edit an item from the TUI and the CLI ([7c09780](https://github.com/bjschnell/Valkyrie/commit/7c097807e5bc70cf02359bc8f3c63cb0d10ee4d7))
* **backlog:** edit an item from the TUI and the CLI ([9bd713c](https://github.com/bjschnell/Valkyrie/commit/9bd713c25d6985655ffc3e08f1dee9bc3c6a0560))
* **backlog:** refuse a blank edit title; show e on done items ([a64ce79](https://github.com/bjschnell/Valkyrie/commit/a64ce796b76f064d3590cd1e467e4a3adc19a8f0))
* **backlog:** send only changed fields from the TUI; let a web clash be saved over ([3d99c01](https://github.com/bjschnell/Valkyrie/commit/3d99c01daa5c787d4bc20b4c6ca1f30f57f78b43))
* **backlog:** the user wins from the TUI and web, and reopening keeps the gate ([cca2dea](https://github.com/bjschnell/Valkyrie/commit/cca2dea2d474bdcd4bed172fc8ec204a4953c54e))
* **context:** ignore a stray .git that git wouldn't take for a repository ([ccd8ffb](https://github.com/bjschnell/Valkyrie/commit/ccd8ffb5428290d93d4f9225c9bcd912ccb513b2))
* **context:** ignore a stray .git that git wouldn't take for a repository ([fb6185e](https://github.com/bjschnell/Valkyrie/commit/fb6185e3e7e2db9fc0db9f24f38a4b32aa8b0db6))
* **context:** skip non-file .git entries; leave a backlog item outside git without a project ([09a2c66](https://github.com/bjschnell/Valkyrie/commit/09a2c6661054dc382e0d52624a439f6480be4370))
* **proto:** don't leave fetch_origin's detach unused on Windows ([de556f8](https://github.com/bjschnell/Valkyrie/commit/de556f8f983b48f8726d721a25c10f87e46fd0fe))
* **tui:** cap a pasted name before Enter saves it; tidy prompt edge cases ([17cf218](https://github.com/bjschnell/Valkyrie/commit/17cf218ec3745abbf6fa22ab81b24bb8d1ccd340))
* **tui:** move the cursor in the rename and backlog prompts; color a new item by kind ([f1db2a5](https://github.com/bjschnell/Valkyrie/commit/f1db2a5c9af590ca88f5dfa7d0bfb4dd5f103f9a))
* **tui:** move the cursor in the rename and backlog prompts; color a new item by kind ([218f2f0](https://github.com/bjschnell/Valkyrie/commit/218f2f0c88a7dbc406e3b07ef082198aa19e3d54))
* **upgrade:** clients follow the daemon's binary; valk upgrade names clients left behind ([bba8e6d](https://github.com/bjschnell/Valkyrie/commit/bba8e6d4652b19838ac5547111b8477c83672631))
* **upgrade:** follow the daemon's binary and name clients left behind ([17ac8d0](https://github.com/bjschnell/Valkyrie/commit/17ac8d0bcf8dfa0db44708d44921631d4eb15147))
* **upgrade:** name only the clients still waited on; treat pid 0 as gone ([85a8256](https://github.com/bjschnell/Valkyrie/commit/85a825684a33f511e3368290f070edb7b7028a15))
* **upgrade:** trust a daemon's binary only from our own user ([93aab44](https://github.com/bjschnell/Valkyrie/commit/93aab444ec2bbf3e0c51c563fac707d8813163b0))
* **worktree:** match squashes byte for byte, whitespace kept; ship deletes a squashed branch ([391fa1e](https://github.com/bjschnell/Valkyrie/commit/391fa1edfb1934bd160c9b941a27dee95c17a81d))
* **worktree:** prune and ship find squash-merged branches ([7d0b10d](https://github.com/bjschnell/Valkyrie/commit/7d0b10dc107bb6ea2391bd4a7d6f6b6ebedf1e11))
* **worktree:** prune finds squash-merged branches by patch id ([134a503](https://github.com/bjschnell/Valkyrie/commit/134a503de57e2efb4eb21ec4ca657e21187e62d9)), closes [#45](https://github.com/bjschnell/Valkyrie/issues/45)

## [0.4.0](https://github.com/bjschnell/Valkyrie/compare/v0.3.0...v0.4.0) (2026-10-29)


### Features

* **cli:** valk pings, a report of where agents interrupt you ([6446479](https://github.com/bjschnell/Valkyrie/commit/6446479ac59511c40681f571fd24032a1b2a54ea))
* **cli:** valk pings, a report of where agents interrupt you ([340b6ca](https://github.com/bjschnell/Valkyrie/commit/340b6cac435842a9ad22ed607d4b7e5b12aa19ed))
* **worktree:** prune merged worktrees with `valk worktree prune` ([6c72f56](https://github.com/bjschnell/Valkyrie/commit/6c72f5681cfe1b9518606a12695ecd35e7ef5a31))
* **worktree:** prune merged worktrees with `valk worktree prune` ([52d8432](https://github.com/bjschnell/Valkyrie/commit/52d8432cfbd659beecb35dab4e2f9ca087624e37))


### Bug Fixes

* **cli:** match deleted worktrees by the --repo path as given ([f8413d6](https://github.com/bjschnell/Valkyrie/commit/f8413d60f1a3bb277d1d91cb0b60ecbb72b3e5f7))
* **cli:** valk pings review fixes ([bb71f78](https://github.com/bjschnell/Valkyrie/commit/bb71f78339e0af02bcea3b93e6752ed0ed596e86))
* **worktree:** tighten prune's merge rule and show what goes ([dead050](https://github.com/bjschnell/Valkyrie/commit/dead05061f23ec5cbba51b104e34f02d427297b9))

## [0.3.0](https://github.com/bjschnell/Valkyrie/compare/v0.2.1...v0.3.0) (2026-10-28)


### Features

* **cli:** add `valk setup alias` for a short `v` command ([50b00d7](https://github.com/bjschnell/Valkyrie/commit/50b00d7310eb584df48351704664975fb823abc8))
* **cli:** add `valk setup alias` for a short `v` command ([ad58633](https://github.com/bjschnell/Valkyrie/commit/ad586330988f289d45817ff999ec7a3ca634dc74))

## [0.2.1](https://github.com/bjschnell/Valkyrie/compare/v0.2.0...v0.2.1) (2026-10-28)


### Bug Fixes

* **context:** never give an agent a proposal's body ([53e5e93](https://github.com/bjschnell/Valkyrie/commit/53e5e93d6b8bbecab417bfdfc303c787c2226891))
* **context:** never give an agent a proposal's body ([3338174](https://github.com/bjschnell/Valkyrie/commit/33381742b0e7d1ce101348024d5acaa323056ea3))

## [0.2.0](https://github.com/bjschnell/Valkyrie/compare/v0.1.1...v0.2.0) (2026-10-28)


### Features

* add a backlog of bugs and ideas that launch agents in worktrees ([476ca8c](https://github.com/bjschnell/Valkyrie/commit/476ca8c47aaa52f830356c0ccc062b7ce43f1a20))
* add a backlog of bugs and ideas that launch agents in worktrees ([8cf2baa](https://github.com/bjschnell/Valkyrie/commit/8cf2baab0785e29f36384a49fb577d6e7dc5ad84))
* **backlog:** agents leave a handoff, and cards show what the checkout holds ([545098e](https://github.com/bjschnell/Valkyrie/commit/545098e7b742ed6b9f91ac45a899a545be8025cd))
* **backlog:** agents leave a handoff, and cards show what the checkout holds ([0ad6b7c](https://github.com/bjschnell/Valkyrie/commit/0ad6b7cfbd95c1afc5f0b51ddea858e6f4ccb79d))
* **context:** propose decisions from what you keep telling agents ([bf72e94](https://github.com/bjschnell/Valkyrie/commit/bf72e94f49e154c9cb8db4da5ad3e55f6c97aefc))
* **context:** propose decisions from what you keep telling agents ([c3613fd](https://github.com/bjschnell/Valkyrie/commit/c3613fde2e5f4b9fc2d482106d5de1b64d755765))
* **context:** propose gotchas and fixes from session failures ([eecf61d](https://github.com/bjschnell/Valkyrie/commit/eecf61d869e5598512f54f24f725f5e6b2ac9b66))
* **context:** propose gotchas and fixes from session failures ([c84f21c](https://github.com/bjschnell/Valkyrie/commit/c84f21c572365ad2f58040754d63b636be21ac49))
* **context:** skills as reviewed decisions, learned from sessions and measured ([9450828](https://github.com/bjschnell/Valkyrie/commit/9450828d6b41e5e4bb01ab2e7a531103de7683a3))
* **context:** skills as reviewed decisions, learned from sessions and measured ([1566509](https://github.com/bjschnell/Valkyrie/commit/1566509336cb170963180d4cac6c08e7d9a10612))
* **context:** skills get learned, triggered, measured and cleaned up on their own ([1ebf4c4](https://github.com/bjschnell/Valkyrie/commit/1ebf4c4356e33006db10c7d5c7692acc27f51730))
* **context:** skills get learned, triggered, measured and cleaned up on their own ([05f8992](https://github.com/bjschnell/Valkyrie/commit/05f89921766f52b0b033d36ae4e15215acdd1b76))
* let a dev daemon's web server take a fixed dev-token ([acf4130](https://github.com/bjschnell/Valkyrie/commit/acf413036196b004f62fd5ccdbef548e1d447142))
* let a dev daemon's web server take a fixed dev-token ([04131a4](https://github.com/bjschnell/Valkyrie/commit/04131a43afb96a358cac253b9fef224c9e71d3be))
* **tui:** a backlog page and a home-screen panel for what waits on you ([e21a72f](https://github.com/bjschnell/Valkyrie/commit/e21a72fcf539e0092fa9d60ab1d3603f199e2586))
* **tui:** a backlog page and a home-screen panel for what waits on you ([f15922b](https://github.com/bjschnell/Valkyrie/commit/f15922b963936298177f5879f2b32cdabb88e3f5))


### Bug Fixes

* **context:** keep a turn's failures until the retro can read them ([52efe7e](https://github.com/bjschnell/Valkyrie/commit/52efe7e5580130fa47d5c1b08bd2c056b6efabb0))
* **daemon:** don't let a detached valk web vouch for itself ([fd1d796](https://github.com/bjschnell/Valkyrie/commit/fd1d796b49571c84e46f5b971eaec1b852265a9e))
* **daemon:** don't let a detached valk web vouch for itself ([829b82f](https://github.com/bjschnell/Valkyrie/commit/829b82fb99c4e301c8ef918d2648db6c33a0db3a))
* **daemon:** start a worktree from its root without a trailing slash ([b6ad18f](https://github.com/bjschnell/Valkyrie/commit/b6ad18f78a6172c858e3cc2fd8ed2798decf6c18))
* **web:** don't dismiss an answer that never arrived; stop long choices overlapping ([18a87f1](https://github.com/bjschnell/Valkyrie/commit/18a87f125f41d58601434e97727c2918a9216a6c))
* **web:** only clear an answered prompt once the keys reach the daemon ([93be242](https://github.com/bjschnell/Valkyrie/commit/93be2425e12b94c194db69c36ceef1c2105a2c2b))

## [0.1.1](https://github.com/bjschnell/Valkyrie/compare/v0.1.0...v0.1.1) (2026-10-27)


### Bug Fixes

* **windows:** stop console windows flashing when the daemon runs git ([b6a19a0](https://github.com/bjschnell/Valkyrie/commit/b6a19a057a1f4fe9bf8c8a0a6643fe053a347d8a))
* **windows:** stop console windows flashing when the daemon runs git ([98e5ae5](https://github.com/bjschnell/Valkyrie/commit/98e5ae5f9ae721dca090fe8454fe9844e7f19b3e))

## [0.1.0](https://github.com/bjschnell/Valkyrie/compare/v0.0.2...v0.1.0) (2026-10-27)


### Features

* **tui:** rename a tab by double-clicking it ([b1e3359](https://github.com/bjschnell/Valkyrie/commit/b1e33593363f01d6b0ee8e091f063adca689a2e1))
* **tui:** rename a tab by double-clicking it ([d7a0dc3](https://github.com/bjschnell/Valkyrie/commit/d7a0dc3a700267f7cf724c367fefb589c5aa0840))

## 0.0.2 and earlier

Before semver; see the git log.
