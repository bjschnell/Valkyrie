# Changelog

Releases from 0.1.0 on follow [Semantic Versioning](https://semver.org) and are
written by release-please from [Conventional Commits](docs/releasing.md).

## [0.5.0](https://github.com/bjschnell/Valkyrie/compare/v0.4.0...v0.5.0) (2026-11-06)


### Features

* **backlog:** adopt an ad-hoc agent session into the backlog ([7128624](https://github.com/bjschnell/Valkyrie/commit/7128624fb7d25eff60c5b9f87559f7e5ca2a1c99))
* **backlog:** adopt an ad-hoc agent session into the backlog ([ae28768](https://github.com/bjschnell/Valkyrie/commit/ae287685da096794474dcec98b200409ddfa31aa))
* **backlog:** agents file, take and track their own work ([af3252b](https://github.com/bjschnell/Valkyrie/commit/af3252b5a8fff933618ee576152daad6db095782))
* **backlog:** filter the backlog by repository ([05316ef](https://github.com/bjschnell/Valkyrie/commit/05316efa6c68dadb8744c0fcc5991a6a27f8e923))
* **backlog:** filter the backlog by repository ([12419e0](https://github.com/bjschnell/Valkyrie/commit/12419e0ffa55daf6019a103dc3891408ba4547ea))
* **backlog:** fix blockers in the change; file the rest, and take it on the user's go-ahead ([079fe46](https://github.com/bjschnell/Valkyrie/commit/079fe46d548a6f7b49d1410410e8522bc02857ee))
* **backlog:** flag a PR that conflicts with its base; ADR-0019 supervisor ([e17990e](https://github.com/bjschnell/Valkyrie/commit/e17990e7ed30a8cf9cace66927f3689c46f7b711))
* **backlog:** flag a session whose PR conflicts with its base; ADR-0019 supervisor ([d3008aa](https://github.com/bjschnell/Valkyrie/commit/d3008aa8dc3e1bdda7e4461d9cc9db953da1b6fc))
* **backlog:** flag an idle session when its item's PR goes red ([be9a667](https://github.com/bjschnell/Valkyrie/commit/be9a6677e3e495467dc253bcb6ed69d9b5e70eaf))
* **backlog:** flag an idle session when its item's PR goes red ([68d5268](https://github.com/bjschnell/Valkyrie/commit/68d52684046eca59d003294c81c3038ab81be3d1))
* **backlog:** launch an item with a chosen model ([fb849ea](https://github.com/bjschnell/Valkyrie/commit/fb849eab364a8651b5053e4c8dde1be54c5f6ff0))
* **backlog:** launch an item with a chosen model ([568e860](https://github.com/bjschnell/Valkyrie/commit/568e860f0b545a40a3431bb1b857bc23f17c6204))
* **backlog:** one session per item, and an open PR shows in review ([0b58a03](https://github.com/bjschnell/Valkyrie/commit/0b58a03f2e99d40edff1798949dfb0a9cccc0307))
* **backlog:** one session per item, PR open shows review, agents track their own work ([9e0b350](https://github.com/bjschnell/Valkyrie/commit/9e0b3504e5d7889c96e84010efe4e26009511b54))
* **backlog:** priority and dependencies decide the order to work items ([3e0db5c](https://github.com/bjschnell/Valkyrie/commit/3e0db5c1d57128e31600307690071af4f973cfb7))
* **backlog:** priority and dependencies decide the order to work items ([3859ecd](https://github.com/bjschnell/Valkyrie/commit/3859ecdbfa432e9d9ebb44d8b684300ab84d8052))
* **backlog:** take an item from a session, ship every merged one at once ([1074773](https://github.com/bjschnell/Valkyrie/commit/1074773055be54d20330a5f9f785ff892a81308f))
* **backlog:** take an item from a session, ship every merged one at once ([6eb328e](https://github.com/bjschnell/Valkyrie/commit/6eb328e25c8ec4a88ea6688795dccca1cc824608))
* **backlog:** tell the session that filed an item when the user accepts it ([9c40623](https://github.com/bjschnell/Valkyrie/commit/9c406236e9f4b21b6e32152995f255399cfb5497))
* **backlog:** tell the session that filed an item when the user accepts it ([75241cc](https://github.com/bjschnell/Valkyrie/commit/75241cc78359f52c419a06ea32577c4bf1e19470))
* **backlog:** the filing agent takes its own find on the user's go-ahead ([94c46fe](https://github.com/bjschnell/Valkyrie/commit/94c46feaabfd861aa4742ed6bab22f217ad03176))
* **backlog:** valk todo plan proposes a build order (ADR-0019 phase 2) ([ed88422](https://github.com/bjschnell/Valkyrie/commit/ed884225ced37c14dfd342756d29af7608c61c92))
* **backlog:** valk todo plan proposes a build order (ADR-0019 phase 2) ([017803e](https://github.com/bjschnell/Valkyrie/commit/017803eae0fc5f7c2dc5b3ab12ac8f70238b7ddb))
* **cli:** edge channel, a rolling build of main for valk upgrade ([594e555](https://github.com/bjschnell/Valkyrie/commit/594e555dcb6991a80f4ecc7faeac443380dabc83))
* **cli:** edge channel, a rolling build of main for valk upgrade ([186b86e](https://github.com/bjschnell/Valkyrie/commit/186b86ed7a75ad1f8f92f882aa54ffcb933f60a4))
* **context:** an ad-hoc session matches its task to an open backlog item ([6bc3e91](https://github.com/bjschnell/Valkyrie/commit/6bc3e91ca7fc8819aea828ef6ca14fc2a86e3a3c))
* **context:** decisions matched to each prompt, and valk decisions search ([78e0f2d](https://github.com/bjschnell/Valkyrie/commit/78e0f2da1aa82b9358c45895722bd4f50b620db2))
* **context:** decisions matched to each prompt, and valk decisions search ([40f819d](https://github.com/bjschnell/Valkyrie/commit/40f819de4aa40eff0c17d10530768fa533a302cb))
* **context:** fix blockers in the change, file the rest for the user ([78b5fd1](https://github.com/bjschnell/Valkyrie/commit/78b5fd1281efbd2046c3c23d8e7e8dc0541f8172))
* **context:** log which decisions reach each session, and flag ones none get ([38fd9bc](https://github.com/bjschnell/Valkyrie/commit/38fd9bcc30ef33fbbf97afddc9a84260c62284ca))
* **context:** log which decisions reach each session, and flag ones none get ([67d1329](https://github.com/bjschnell/Valkyrie/commit/67d1329438e2f8f13668616df1c4152bb25fc118))
* **context:** make fixing what agents find the default; filing names why ([f61c28d](https://github.com/bjschnell/Valkyrie/commit/f61c28dd840fff0856467d57e1c63e6d018a3ec2))
* **context:** scope decisions to paths and serve them per file (ADR-0014 phase 4) ([47e7590](https://github.com/bjschnell/Valkyrie/commit/47e7590ce09066fd2b873bd5f1c52361696654ff))
* **context:** scope decisions to paths and serve them per file (ADR-0014 phase 4) ([6006271](https://github.com/bjschnell/Valkyrie/commit/600627126f1edf5c1fc0b3cc93359bfc020ab68f))
* **context:** the session-start block as an index, with pins only you set ([abb2217](https://github.com/bjschnell/Valkyrie/commit/abb2217ad71dd9c0b8f0a49e5fcd26625851c1f2))
* **context:** the session-start block as an index, with pins only you set ([f8b1210](https://github.com/bjschnell/Valkyrie/commit/f8b12106a3b44bba57d893f9d4a952feb69d28a0))
* **daemon:** ship a backlog item when its pull request merges ([15a2841](https://github.com/bjschnell/Valkyrie/commit/15a2841e5bb43f80884e76f3bc6dc7f01ac198fb))
* **daemon:** ship a backlog item when its pull request merges ([ffa2b03](https://github.com/bjschnell/Valkyrie/commit/ffa2b03305f1e44154d48bcb2a395fc9826e77cd))
* **hook:** merge guard asks before an agent merges or pushes to the default branch ([6e4feff](https://github.com/bjschnell/Valkyrie/commit/6e4fefff6e570efb17a4d3437b5285e2364a7691))
* **hook:** merge guard asks before an agent merges or pushes to the default branch ([8bd98e8](https://github.com/bjschnell/Valkyrie/commit/8bd98e86aa4da5c75d0cad68a43b919ddd78e1aa))
* name sessions after what their agent is working on ([8595555](https://github.com/bjschnell/Valkyrie/commit/8595555ac75e0b5727ca0c402310777e25e1fd21))
* name sessions after what their agent is working on ([f4fab3f](https://github.com/bjschnell/Valkyrie/commit/f4fab3ff8b0cec4588729e48d9db1143cf524e9d))
* **pings:** record where each turn left its work and count stalls ([0a932bf](https://github.com/bjschnell/Valkyrie/commit/0a932bf3fa42d67b167d0013d757dd7353b8a1c5))
* **pings:** record where each turn left its work and count stalls ([ce68308](https://github.com/bjschnell/Valkyrie/commit/ce683087dda63205532bb225dd4bf91fd156116f))
* **policy:** a red PR past ci-fix's limit shows as blocked, not finished ([77f23cf](https://github.com/bjschnell/Valkyrie/commit/77f23cf0c9f71560df55e3fa09f1aaff70ba51e6))
* **policy:** a red PR past ci-fix's limit shows as blocked, not finished ([608ae4e](https://github.com/bjschnell/Valkyrie/commit/608ae4e70f5d2fecbef8d3a2c49341c464509944))
* **policy:** a ship policy keeps Claude's turn going until work is shipped ([6bb6867](https://github.com/bjschnell/Valkyrie/commit/6bb68670fbe57ef07415628ff050fa5d67deeeec))
* **policy:** a ship policy keeps Claude's turn going until work is shipped ([408d809](https://github.com/bjschnell/Valkyrie/commit/408d809ee86885cc46539af280f6cc225f5ab63a))
* **policy:** a user scope for habits, laid under each project's settings ([6e96390](https://github.com/bjschnell/Valkyrie/commit/6e96390bdbd1d926b0add3953f852b5fad760e21))
* **policy:** ci-fix keeps an agent on a red pull request (ADR-0013 phase 4) ([ef6fb7b](https://github.com/bjschnell/Valkyrie/commit/ef6fb7b951016f972e1ffad9b316bd6269b0fd17))
* **policy:** ci-fix keeps an agent on a red pull request (ADR-0013 phase 4) ([db0f5eb](https://github.com/bjschnell/Valkyrie/commit/db0f5ebf0de914d0054d8488bdd9933e0a8da0a0))
* **policy:** health flags a policy overridden in 3+ sessions ([501da08](https://github.com/bjschnell/Valkyrie/commit/501da08a464e6b424af920c2cd904559fee09a56))
* **policy:** health flags a policy overridden in 3+ sessions ([f5d65b4](https://github.com/bjschnell/Valkyrie/commit/f5d65b44084428b2b42585885b256779a304cd35))
* **policy:** learn a repo's checks and ci: agent from what runs in it ([#47](https://github.com/bjschnell/Valkyrie/issues/47)) ([cf4aa28](https://github.com/bjschnell/Valkyrie/commit/cf4aa283a0b66880e03eab5233848f70489c142b))
* **policy:** learn a repository's checks and ci: agent from what runs in it ([038338b](https://github.com/bjschnell/Valkyrie/commit/038338bc3a366dd54d5971e3ab2906e8a9a6babc))
* **policy:** learn the ship habit from how you answer stops ([cc745a4](https://github.com/bjschnell/Valkyrie/commit/cc745a44184aa143742fc918b3653eeb3b91f11f))
* **policy:** learn the ship habit from how you answer stops ([#37](https://github.com/bjschnell/Valkyrie/issues/37)) ([88a2431](https://github.com/bjschnell/Valkyrie/commit/88a2431a73a663b5bce163e73009567e12a7d3f3))
* **policy:** user-scope ship habit under each project's settings ([#46](https://github.com/bjschnell/Valkyrie/issues/46)) ([b77289b](https://github.com/bjschnell/Valkyrie/commit/b77289bedc7c67c75a843f0dc08ffe2d14db1d8e))
* **supervisor:** a flagged session offers merge-base, ci-fix or rerun (ADR-0019 phase 2) ([d668ccf](https://github.com/bjschnell/Valkyrie/commit/d668ccfdecf2670b64e450f9f5f464f7a41a67b9))
* **supervisor:** a flagged session offers merge-base, ci-fix or rerun (ADR-0019 phase 2) ([fe1e880](https://github.com/bjschnell/Valkyrie/commit/fe1e8803394d88907f79350769902c01266d6654))
* **supervisor:** accepted steps become policies that run on their own (ADR-0019 phase 3) ([0f8d746](https://github.com/bjschnell/Valkyrie/commit/0f8d74627f7493cbd32a8662ddff31719883d2a8))
* **supervisor:** launch the next ready item when there's room (ADR-0019 phase 4) ([f009fcb](https://github.com/bjschnell/Valkyrie/commit/f009fcb33fb795c7f3d86c125fdece3acd5085d1))
* **supervisor:** launch the next ready item when there's room (ADR-0019 phase 4) ([a756d25](https://github.com/bjschnell/Valkyrie/commit/a756d25cacded9c91f4c924f95cd64087c88845b))
* **supervisor:** watch PRs of sessions with no backlog item (ADR-0019 phase 5) ([fccec1a](https://github.com/bjschnell/Valkyrie/commit/fccec1a9a62aaf0abee9911aa11a7b6ff7b7c5f2))
* **supervisor:** watch PRs of sessions with no backlog item (ADR-0019 phase 5) ([364eeef](https://github.com/bjschnell/Valkyrie/commit/364eeef2dbe517f04a64d849f689d81ca6775bf5))
* **tui:** Alt-a captures a backlog item from anywhere ([b43f946](https://github.com/bjschnell/Valkyrie/commit/b43f946499904a8e48d9c92f217b54979510cffc))
* **tui:** Alt-a captures a backlog item from anywhere ([5e2e393](https://github.com/bjschnell/Valkyrie/commit/5e2e39381df807d88dfbc6bb1a6afd81c071d5a4))
* **tui:** mini terminal over any screen on Alt-m ([c13ff77](https://github.com/bjschnell/Valkyrie/commit/c13ff771665248015117ec300d7dc0f75302aeb9))
* **tui:** mini terminal over any screen on Alt-m ([5a5e37b](https://github.com/bjschnell/Valkyrie/commit/5a5e37b7a9d8db330113b456abd2a30d5191e55f))


### Bug Fixes

* **backlog:** a deleted blocker no longer blocks editing what waited on it ([9c8d0bf](https://github.com/bjschnell/Valkyrie/commit/9c8d0bfc9af78672aa177d9fbfa47760d91f2d77))
* **backlog:** a new PR flag replaces the poll's own earlier one ([a216cb0](https://github.com/bjschnell/Valkyrie/commit/a216cb0b9f5ad4d26a1fbcff364e63277571e967))
* **backlog:** a session is on the item it took last, not the one it last noted ([d2bef55](https://github.com/bjschnell/Valkyrie/commit/d2bef5585efc014944bb1a9c436068ec37161ed4))
* **backlog:** a session is on the item it took last, not the one it last noted ([f5ff965](https://github.com/bjschnell/Valkyrie/commit/f5ff9656d2db87292ff5dfcd7c327d99e84ab0eb))
* **backlog:** a shipped item stays the user's to drop ([397d682](https://github.com/bjschnell/Valkyrie/commit/397d682548271b1d1ecf093558108db524283c02))
* **backlog:** a take never locks the user out of an item ([a232f5d](https://github.com/bjschnell/Valkyrie/commit/a232f5d2e410e09ab238c4c02e772b41f9691293))
* **backlog:** after a web clash, move on only the clashing fields ([e39da1f](https://github.com/bjschnell/Valkyrie/commit/e39da1f89c2967e8b041f3aa549c92df1605363e))
* **backlog:** agents drop or reword only the cards they filed ([9997a10](https://github.com/bjschnell/Valkyrie/commit/9997a1015bec0120e7b05a4919a9448affd589c8))
* **backlog:** agents drop or reword only the cards they filed ([0854aee](https://github.com/bjschnell/Valkyrie/commit/0854aeef37ffc0cbc4e878dc93b11e84708f86e8))
* **backlog:** an edit changes only the fields it gives ([cfd77f5](https://github.com/bjschnell/Valkyrie/commit/cfd77f5c2c0c21c5a1bf6f9cf8036d6fbbb09bb3))
* **backlog:** an edit changes only the fields it gives ([03ddf1e](https://github.com/bjschnell/Valkyrie/commit/03ddf1ef05c1de7a09e756d94184f2b53ca268eb))
* **backlog:** an interrupted session stays in progress, not review ([0a11eae](https://github.com/bjschnell/Valkyrie/commit/0a11eae9f375dc4357d78368d47bedae488f15ba))
* **backlog:** an interrupted session stays in progress, not review ([8d0987c](https://github.com/bjschnell/Valkyrie/commit/8d0987cb8e06660f25bfb38c203b2619d53522ee)), closes [#23](https://github.com/bjschnell/Valkyrie/issues/23)
* **backlog:** board Launch starts the agent the item last ran ([705747f](https://github.com/bjschnell/Valkyrie/commit/705747f62e1c7005e15cac168fd829cf0a865681))
* **backlog:** board Launch starts the agent the item last ran ([1df40da](https://github.com/bjschnell/Valkyrie/commit/1df40da287d21a57dc43b64dbcd84cc714b6e5b6))
* **backlog:** claim/accept edge cases from [#55](https://github.com/bjschnell/Valkyrie/issues/55) review ([3f6956b](https://github.com/bjschnell/Valkyrie/commit/3f6956b78d7f6cc9edfaf70de22afe6dfc33562d))
* **backlog:** edit an item from the TUI and the CLI ([7c09780](https://github.com/bjschnell/Valkyrie/commit/7c097807e5bc70cf02359bc8f3c63cb0d10ee4d7))
* **backlog:** edit an item from the TUI and the CLI ([9bd713c](https://github.com/bjschnell/Valkyrie/commit/9bd713c25d6985655ffc3e08f1dee9bc3c6a0560))
* **backlog:** judge an item change on the item as it's written ([d0df4be](https://github.com/bjschnell/Valkyrie/commit/d0df4be8df7c0345cdf85598d79f2490ba075649))
* **backlog:** judge an item change on the item as it's written ([3716462](https://github.com/bjschnell/Valkyrie/commit/37164622f08b9f9883f2687ea0ed5003fd0a8517))
* **backlog:** label an item whose PR closed unmerged "PR closed" ([ea75717](https://github.com/bjschnell/Valkyrie/commit/ea757174635941ffe84350756ecfde3c3f7946e4))
* **backlog:** label an item whose PR closed unmerged "PR closed" ([474c61b](https://github.com/bjschnell/Valkyrie/commit/474c61b5e27e0d710b1d7f06d974e670a9d43885))
* **backlog:** Proposed clears the unaccepted flag; an agent's own session is told it's already on the item ([c4ced69](https://github.com/bjschnell/Valkyrie/commit/c4ced695be632dc928397b4052dc0ac7f2f2829c))
* **backlog:** refuse a blank edit title; show e on done items ([a64ce79](https://github.com/bjschnell/Valkyrie/commit/a64ce796b76f064d3590cd1e467e4a3adc19a8f0))
* **backlog:** send only changed fields from the TUI; let a web clash be saved over ([3d99c01](https://github.com/bjschnell/Valkyrie/commit/3d99c01daa5c787d4bc20b4c6ca1f30f57f78b43))
* **backlog:** the user wins from the TUI and web, and reopening keeps the gate ([cca2dea](https://github.com/bjschnell/Valkyrie/commit/cca2dea2d474bdcd4bed172fc8ec204a4953c54e))
* **cli:** end quietly when the stdout reader goes away ([51e04c6](https://github.com/bjschnell/Valkyrie/commit/51e04c6c279754513c8f349a00cef268dccf20a6))
* **cli:** end quietly when the stdout reader goes away ([ae90678](https://github.com/bjschnell/Valkyrie/commit/ae9067868a4534065948d5e7504024696a107b66))
* **context:** a pin isn't a confirmation, and follows the rule, not its wording ([bdcfc42](https://github.com/bjschnell/Valkyrie/commit/bdcfc426ba919b4b6ee0c8a3b9ea7ee2167fe9b1))
* **context:** compare a file to its checkout in the same canonical form on Windows ([b2613f7](https://github.com/bjschnell/Valkyrie/commit/b2613f7d4bdaee13f2b4b93f8f47dab797211344))
* **context:** every active decision reaches the agent, at least by title ([4fb9da0](https://github.com/bjschnell/Valkyrie/commit/4fb9da096e0aaa4f273d1962da2be2d1dd4aa6b6))
* **context:** every active decision reaches the agent, at least by title ([8ce74d3](https://github.com/bjschnell/Valkyrie/commit/8ce74d31b6f6ae723a673ab6a00429dc2f3368d9))
* **context:** ignore a stray .git that git wouldn't take for a repository ([ccd8ffb](https://github.com/bjschnell/Valkyrie/commit/ccd8ffb5428290d93d4f9225c9bcd912ccb513b2))
* **context:** ignore a stray .git that git wouldn't take for a repository ([fb6185e](https://github.com/bjschnell/Valkyrie/commit/fb6185e3e7e2db9fc0db9f24f38a4b32aa8b0db6))
* **context:** lock served.jsonl so the weekly prune can't drop another session's serves ([d3ecfca](https://github.com/bjschnell/Valkyrie/commit/d3ecfca7bdce0939307588a9bfea264e2c62564d))
* **context:** lock served.jsonl so the weekly prune can't drop another session's serves ([5d078a7](https://github.com/bjschnell/Valkyrie/commit/5d078a7f8338ccba419f8a41518e9679424e1058))
* **context:** skip non-file .git entries; leave a backlog item outside git without a project ([09a2c66](https://github.com/bjschnell/Valkyrie/commit/09a2c6661054dc382e0d52624a439f6480be4370))
* **context:** skip subagents' file reads, match globs without backtracking, show a proposal's scope in the TUI ([6e569a1](https://github.com/bjschnell/Valkyrie/commit/6e569a16e132285aa12d93e68ae819099ad6b434))
* **context:** weigh prompt matches by rarity, and serve only what's logged ([671683f](https://github.com/bjschnell/Valkyrie/commit/671683fa1805e13957909e2f755cb6c509deabd7))
* **daemon:** only the agent's own turn end starts a read or releases replies ([de3ec28](https://github.com/bjschnell/Valkyrie/commit/de3ec28c15ed31707ef74f33d9cace5ece26b1e7))
* **daemon:** pair a reply typed while the stop's work state is read ([edd2325](https://github.com/bjschnell/Valkyrie/commit/edd2325e37b870f04fab09185b1516b292ec0c4e))
* **daemon:** pair a reply typed while the stop's work state is read ([58b0161](https://github.com/bjschnell/Valkyrie/commit/58b0161c1d97ec7e6c993aac3088ea9144c7f56e))
* **daemon:** two daemons on one state dir keep their own orders and reruns ([b308dd1](https://github.com/bjschnell/Valkyrie/commit/b308dd19191f92e15c2a6647781ca5381d3dbdd9))
* **decisions:** an empty habit list suggests recording a habit ([e27449e](https://github.com/bjschnell/Valkyrie/commit/e27449e08a96d2828108a01b59c0468c7f134ab2))
* **decisions:** an empty habit list suggests recording a habit ([713543e](https://github.com/bjschnell/Valkyrie/commit/713543ea0dcc2516a0473e8a13ef4e2efd1334e5))
* **hook:** read the whole payload on every path, so the agent never hits EPIPE ([6884b9a](https://github.com/bjschnell/Valkyrie/commit/6884b9afb93ad16e1dd98ca2e8b2b8fffd4e5c07))
* **hook:** read the whole payload on every path, so the agent never hits EPIPE ([f4bda90](https://github.com/bjschnell/Valkyrie/commit/f4bda90fb3c839dfc81151b2fbb95298215a69d4))
* **install:** rebuild the checkout's own valk instead of adding a second one ([e4b3d65](https://github.com/bjschnell/Valkyrie/commit/e4b3d65173811f0da00c7beee247514d73736167))
* **install:** rebuild the checkout's own valk instead of adding a second one ([b2464ae](https://github.com/bjschnell/Valkyrie/commit/b2464aea8b61c90995c3ed9b5d077e94a41727e3))
* **install:** spell the PATH lookup as an if, for shellcheck SC2015 ([4fef297](https://github.com/bjschnell/Valkyrie/commit/4fef2973e12e573fcbc845dc18d3e69c08814ccf))
* **policy:** ci-fix reads only the repository's ci, checks its limit as it counts ([cefcf84](https://github.com/bjschnell/Valkyrie/commit/cefcf842f202fbd24856632940aa7675e8f24710))
* **policy:** count ship-habit replies by session, not session name ([c7bbc5e](https://github.com/bjschnell/Valkyrie/commit/c7bbc5e0c827a834ac4c8303ffda1c779cf4b7b6))
* **policy:** count ship-habit replies by session, not session name ([#64](https://github.com/bjschnell/Valkyrie/issues/64)) ([40e21c8](https://github.com/bjschnell/Valkyrie/commit/40e21c86bc49a389dab0252f322950472e05c124))
* **policy:** keep an override reply across a re-block; narrow gave_up ([0fea670](https://github.com/bjschnell/Valkyrie/commit/0fea67016b49d85717a41626e102f0ab9ca55aef))
* **policy:** read a stop's work state from the session's taken item ([e4315a8](https://github.com/bjschnell/Valkyrie/commit/e4315a81cffd28fe8a78e79f62b3ef81aa69b64d))
* **policy:** read a stop's work state from the session's taken item ([4993d2f](https://github.com/bjschnell/Valkyrie/commit/4993d2fba99f469c70fde0a466fece558a1b1e34))
* **proto:** don't leave fetch_origin's detach unused on Windows ([de556f8](https://github.com/bjschnell/Valkyrie/commit/de556f8f983b48f8726d721a25c10f87e46fd0fe))
* **review:** name the user scope "your habits", not ~user ([adf3f8e](https://github.com/bjschnell/Valkyrie/commit/adf3f8ef8fa11420efeb1bfc8b1af048e322c09d))
* **review:** name the user scope "your habits", not ~user ([#62](https://github.com/bjschnell/Valkyrie/issues/62)) ([92b10b4](https://github.com/bjschnell/Valkyrie/commit/92b10b4d6737518cd0a5a54d9bf0cdad2b25ceba))
* **ship:** a pushed branch whose commits came by rebase counts as work ([f719b1b](https://github.com/bjschnell/Valkyrie/commit/f719b1b71ec583394311081506d51920b22ff5aa))
* **ship:** a pushed branch whose commits came by rebase counts as work ([6120299](https://github.com/bjschnell/Valkyrie/commit/61202995788c458fb7e536a9d9294c31f4da1d59))
* **supervisor:** a run already rerun spends the rerun, and ADR-0019 matches the code ([7929cf7](https://github.com/bjschnell/Valkyrie/commit/7929cf7e3e0c91b4cf77a5dac00fddfd8c64bdef))
* **supervisor:** a run already rerun spends the rerun, and ADR-0019 matches the code ([21a6e1a](https://github.com/bjschnell/Valkyrie/commit/21a6e1a0d82835dd41227fdce11e8eedf5f8f36a))
* **tui:** a stray empty .git no longer counts as a repository ([3105b0e](https://github.com/bjschnell/Valkyrie/commit/3105b0e76f1d1f63eb21045dea6cc1cfe0af81fa))
* **tui:** a stray empty .git no longer counts as a repository ([cbae25e](https://github.com/bjschnell/Valkyrie/commit/cbae25e530bbd61ea9c07006b7439f7e6c85cb09))
* **tui:** cap a pasted name before Enter saves it; tidy prompt edge cases ([17cf218](https://github.com/bjschnell/Valkyrie/commit/17cf218ec3745abbf6fa22ab81b24bb8d1ccd340))
* **tui:** harden the backlog prompt's escape handling ([b1b938b](https://github.com/bjschnell/Valkyrie/commit/b1b938b65c62e56369bc6e4d6b26335fbb52a1d9))
* **tui:** harden the backlog prompt's escape handling ([1dd4f59](https://github.com/bjschnell/Valkyrie/commit/1dd4f59e06c47b4197a444c07d23a63548223306))
* **tui:** move the cursor in the rename and backlog prompts; color a new item by kind ([f1db2a5](https://github.com/bjschnell/Valkyrie/commit/f1db2a5c9af590ca88f5dfa7d0bfb4dd5f103f9a))
* **tui:** move the cursor in the rename and backlog prompts; color a new item by kind ([218f2f0](https://github.com/bjschnell/Valkyrie/commit/218f2f0c88a7dbc406e3b07ef082198aa19e3d54))
* **tui:** show each backlog row's state in a column ([41dd049](https://github.com/bjschnell/Valkyrie/commit/41dd049ce6312e09326d84cac65158cd86e194e7))
* **tui:** show each backlog row's state in a column ([92c90ca](https://github.com/bjschnell/Valkyrie/commit/92c90cabf23467516bc0bc96da5a78f7eb2d1ea1))
* **upgrade:** clients follow the daemon's binary; valk upgrade names clients left behind ([bba8e6d](https://github.com/bjschnell/Valkyrie/commit/bba8e6d4652b19838ac5547111b8477c83672631))
* **upgrade:** follow the daemon's binary and name clients left behind ([17ac8d0](https://github.com/bjschnell/Valkyrie/commit/17ac8d0bcf8dfa0db44708d44921631d4eb15147))
* **upgrade:** name only the clients still waited on; treat pid 0 as gone ([85a8256](https://github.com/bjschnell/Valkyrie/commit/85a825684a33f511e3368290f070edb7b7028a15))
* **upgrade:** trust a daemon's binary only from our own user ([93aab44](https://github.com/bjschnell/Valkyrie/commit/93aab444ec2bbf3e0c51c563fac707d8813163b0))
* **web:** chat shows an agent's clarifying question in full ([1dd07c9](https://github.com/bjschnell/Valkyrie/commit/1dd07c99c7bc71d335cbb60ff0242f7e13aa1d69))
* **web:** chat shows an agent's clarifying question in full ([c1978ef](https://github.com/bjschnell/Valkyrie/commit/c1978ef99166eaafcfc2d1e1200b1fbf0d480245))
* **web:** speak protocol 19 so the app lists sessions again ([15ee830](https://github.com/bjschnell/Valkyrie/commit/15ee8302f5dc83929142bd419ccbd39efc71aa4d))
* **web:** speak protocol 19 so the app lists sessions again ([10f4b0f](https://github.com/bjschnell/Valkyrie/commit/10f4b0f41f3078edf98179d97aa60e699e4fef83))
* **web:** take the app's protocol from valk web, and say loudly when it's outdated ([02f84de](https://github.com/bjschnell/Valkyrie/commit/02f84def13c82185eee28bf22c17a4e30b84bb89))
* **web:** take the app's protocol from valk web, and say loudly when it's outdated ([2b0d096](https://github.com/bjschnell/Valkyrie/commit/2b0d0966edaebaed99eb1cf8a25bdd69d7876e64))
* **windows:** an elevated terminal gets a daemon of its own ([cfb287b](https://github.com/bjschnell/Valkyrie/commit/cfb287bea10121c9a1eea43fd306f181862fe618))
* **windows:** keep elevated and unelevated daemons from crossing ([fce651c](https://github.com/bjschnell/Valkyrie/commit/fce651ce2c3558ae3335baa1a4eea38a46bbdc43))
* **work:** a check cancelled in one run and passed in another isn't red ([4da3f51](https://github.com/bjschnell/Valkyrie/commit/4da3f5128442853b838dc71700c62af12e5110c8))
* **work:** a squash-merged branch whose remote is gone isn't unpushed ([b87c2f3](https://github.com/bjschnell/Valkyrie/commit/b87c2f37629e487854d418c4caef2ed0321c6047))
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
