# Repository wiki

The wiki is a set of pages agents write for agents about how a repository is built: its parts, how data and control move between them, the rules the code relies on, and how to make the usual kind of change. Without it, every new session explores the same code again before it can design anything. With it, the agent reads the pages its task needs and starts from there.

```sh
valk wiki explore           # start an agent that maps this repository into pages
valk wiki explore --stale   # start one that rewrites only the stale pages
valk wiki explore --print   # print the brief instead (--agent codex, -a to attach)
valk wiki                   # list the pages, stale ones marked
valk wiki show daemon       # one page, with what it covers and its commit
valk wiki rm daemon         # delete a page
```

From the apps:

- **Web app:** the **Wiki** tab in **Memory** (`#/memory/wiki`). Pick a repository to see its pages, stale ones marked, and tap one to read it. **Map this repo** (or **Explore again**) and **Refresh N stale** start the agent. Each asks for a second tap, since it starts an agent, then opens the new session.
- **TUI:** `W` on the home screen opens the wiki of the selected session's repository, or the one the TUI started in. `↩` reads a page. `e` maps the repository, `S` rewrites the stale pages, and each one waits for `↩` to confirm.

Starting it again while the agent is still running opens that session instead of starting a second one.

How it works:

1. **You start it.** Nothing explores on its own. The apps' buttons and `valk wiki explore` all start a session named `wiki` in the repository's root, with a brief. The brief asks for an `overview` page first, then one page per part (a crate, package, service or layer) and one per cross-cutting concern (a protocol, storage, configuration, testing), usually 5 to 15 pages. When pages already exist, the agent reads them first and fills the gaps. The agent doesn't change the repository.
2. **Agents write pages with `valk wiki write`.** `valk wiki write <page> --title T --summary S --scope PATH... -` reads the markdown from stdin. The MCP tool `write_wiki_page` does the same. Writing a page that already exists replaces it. Any agent that finds a page wrong can rewrite it.
3. **Every session gets the index.** At session start, after the decisions block, an agent gets one line per page: its name, title and summary, with stale pages marked. That index is capped at 1,500 characters. Agents read full pages on demand with `valk wiki show`, or with `list_wiki` and `get_wiki_page` over MCP, so a wiki never fills the context on its own.
4. **Pages go stale.** Each page records the commit it was written at and the paths it covers (`--scope`). A page goes stale when one of those paths is deleted, or when 300 lines or more change under them since that commit, counted as notes count it (a worktree's own branch edits don't count). `valk wiki explore --stale` hands just those pages to an agent to rewrite.

Pages live in `<state dir>/context/<project>/wiki/<page>.md`, shared by every worktree of the repository and never written into it. A page name is lowercase letters, digits and dashes. A page holds at most 16,000 characters and covers at most 12 paths, and a repository keeps at most 40 pages. Scope paths must exist inside the repository.

The wiki is what the code looks like. [Decisions](decisions.md) are rules you've accepted, and [notes](notes.md) are findings about what broke. The wiki isn't reviewed, so the index tells agents to check a page against the code before relying on it.
