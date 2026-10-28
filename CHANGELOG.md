# Changelog

Releases from 0.1.0 on follow [Semantic Versioning](https://semver.org) and are
written by release-please from [Conventional Commits](docs/releasing.md).

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
