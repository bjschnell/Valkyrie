# Settings

Press `,` to open the settings panel. You can change:

- the theme: `dracula`, `cyberpunk`, `blackout`, `catppuccin`, `nord`, `gruvbox`,
  `tokyonight`, `solarized`, `onedark`, `rosepine`, `monokai`, `solarizedlight`,
  `catppuccinlatte`, `gruvboxlight`, `matrix`, `violet`, `valkyrie` or `netrunner`
- whether sessions take the theme's colors (`theme`) or your terminal's
  (`terminal`)
- the tab style (`underline`, `folder`, `cards`), side (`top`, `left`, `right`)
  and detail (`full`, or `minimal`: one row per tab; at the side, a rail of numbers
  and state icons that opens over the session under the mouse)
- key hints (`on`/`off`, off by default): on, the bar under a session also shows `^\ actions` and `alt h home`. Off, it says where every key is instead (`^\ ? keys`); click that, or press the actions key then `?`, for the sheet.
  Actions (`^\`) show their keys either way
- the ping sound (`chime`, `pop`, `wood`, `beep`, 8-bit `coin`, `powerup`, `zap`,
  `1up`, or `none`), desktop notifications for toasts (`on`/`off`), and project context
- proposals (`on`/`off`): an agent's new backlog item pops up as a toast. While it's
  up, tab mode `a` accepts it, `x` declines it and `b` opens the backlog on it, as
  a click on it does. The web app has its own switch, under Settings → Agent proposals
- analytics (`on`/`off`, off by default): count which features get used, for
  `valk stats` ([usage](usage.md#is-it-helping)). It stays on this machine

With `theme` colors, a session's background, default text and 16 ANSI colors come
from the theme. Colors a program picks exactly (256-color or RGB) stay as it drew
them.

## Theme packs

A theme pack is a look that lives on one machine instead of in Valkyrie: a
folder in `~/.config/valkyrie/themes/` (`%APPDATA%\Valkyrie\themes\` on Windows)
named for the theme, lowercase letters, digits and dashes. valk and the web app
offer it beside the built-in themes on that machine only; restart valk, or
reload the web app, to pick up a new one.

Its `theme.toml` takes `key = value` lines, like `settings.toml`:

```toml
about = "Neon on black"   # the line under its name in the pickers
bg = #07050d              # required, with fg and accent
fg = #efe6ff
accent = #c084fc
muted = #8a74b3           # optional: panel, strip, card, selection, border,
needs = #ffe36e           #   needs, blocked, done, working; the rest are worked
ansi = "#140f22 #ff4d6d"  #   out from these. Up to 16 ANSI colors, in order
rain.glyphs = "01ABCDEF"  # rain: its own glyphs, or code, hex or runes
rain.head = #f6e5ff
rain.trail = "#a855f7 #6b21a8 #2a0f45"
web.pack-opacity = 0.85   # web.<name> sets that CSS variable in the web app
```

A pack can also bring a figure made of its rain, which follows your sessions:
formed while something needs you, gathering while agents work, latent
otherwise. No built-in theme reacts to state. For the TUI that's
`tui/avatar.json`, frames of glyphs at a few sizes. For the web app it's
`web/index.js`, an ES module exporting `mount(el, { base, glyphs })`, which
returns `{ update(state), destroy() }`. `state` is `{ needs, working,
workingSince }`, and the module's other files are served from `base`, but only
to paired devices.

## Where settings are kept

Settings are kept in `~/.config/valkyrie/settings.toml`
(`%APPDATA%\Valkyrie\settings.toml` on Windows), which you can also edit by hand.
`VALK_THEME`, `VALK_SESSION_COLORS`, `VALK_TABS`, `VALK_TAB_SIDE`, `VALK_TAB_DETAIL`, `VALK_SOUND`, `VALK_DESKTOP`, `VALK_PROPOSALS` and
`VALK_PROJECT_CONTEXT` and `VALK_ANALYTICS` override it for one run.
