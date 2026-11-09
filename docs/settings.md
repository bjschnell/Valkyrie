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
- key hints (`on`/`off`): whether the bar under a session lists its keys.
  Actions (`^\`) show their keys either way
- the ping sound (`chime`, `pop`, `wood`, `beep`, 8-bit `coin`, `powerup`, `zap`,
  `1up`, or `none`), and project context

With `theme` colors, a session's background, default text and 16 ANSI colors come
from the theme. Colors a program picks exactly (256-color or RGB) stay as it drew
them.

## Where settings are kept

Settings are kept in `~/.config/valkyrie/settings.toml`
(`%APPDATA%\Valkyrie\settings.toml` on Windows), which you can also edit by hand.
`VALK_THEME`, `VALK_SESSION_COLORS`, `VALK_TABS`, `VALK_TAB_SIDE`, `VALK_TAB_DETAIL`, `VALK_SOUND` and
`VALK_PROJECT_CONTEXT` override it for one run.
