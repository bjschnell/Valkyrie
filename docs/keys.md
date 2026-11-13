# Keys and the mouse

A few keys get you started: `↩` on the home screen opens a session, `^]`
or `Alt-h` takes you home, and `Alt-n` steps through each session that needs you.
Rest the mouse on a tab for a peek at its agent's state, summary and screen.
In a session, `^\` is actions, for what the Alt keys don't do: split, rename,
decisions. The bar under a session lists these keys, and you can click them.

## All default keys, remapping, and the mouse

| Where | Key | Does |
|---|---|---|
| Home | `↩` | open the selected session |
| | `n` · `x` · `r` | new shell · kill · rename |
| | `,` · `.` | settings · keys |
| Anywhere | `Alt-1`–`Alt-9` · `Alt-h` | jump to a tab · home |
| | `Alt-j` / `Alt-k` · `Alt-p` | next / previous tab · the tab you were last on |
| | `Alt-n` | each session that needs you, then back where you started |
| | `Alt-t` · `Alt-w` · `Alt-o` | new shell tab · close tab · next pane |
| | `Alt-z` | zoom: the pane you're in fills the screen, `Alt-z` again shows the split; `Alt-o` while zoomed shows the next pane full-size |
| | `Alt-a` | add a bug or idea into the backlog (`↩` adds, `Esc` cancels), then back where you were |
| | `Alt-m` | mini terminal: a small shell in a box over the screen, for a side command; `Alt-m` again puts it away, and it keeps running until you `exit` it |
| Attached | `^\` | actions (below) |
| | `^]` | back home |
| Actions | `s` / `v` · `p` · `z` | split below / beside · the pane's menu · zoom |
| | `r` · `,` | rename the tab · settings |
| | `m` | minimal tabs: one row each; at the side, a rail of icons that opens under the mouse |
| | `d` | decisions to review, over the session (`a`/`d` accept/reject) |

**Remapping.** Press `.` on the home screen (or in the settings panel), then `↩`
and the new key. `⌫` clears a key, `r` puts back its default, and `R` twice puts
back every default. A key can be Alt and almost any key, or Ctrl and a letter,
apart from the ones programs need (Ctrl-C, Ctrl-D, Ctrl-Z, …). `^]` always goes
home, so a remap can't lock you out. Changes are kept in `settings.toml` as
`keys.<action> = "<key>"`.

**On a Mac,** the Alt keys are Option, once your terminal sends Option as Meta.
Until it does, Option types `˙` and friends, and the bar says where to change it.
Setting only the left Option key keeps the right one for `é`, `€` and the like.

| Terminal | Setting |
|---|---|
| Terminal.app | Settings → Profiles → Keyboard → Use Option as Meta key |
| iTerm2 | Settings → Profiles → Keys → Left Option key: Esc+ |
| Ghostty | `macos-option-as-alt = left` |
| kitty | `macos_option_as_alt left` |
| WezTerm | `send_composed_key_when_left_alt_is_pressed = false` |

**The mouse.** Rest the pointer on a tab to peek at it. Click a tab, drag it to reorder, right-click it for a menu, right-click a pane to split
it, and drag a divider to resize. The key chips on the bar under a session, and
`◇ N to review`, click like the keys. Click a link in a
session to open it in your browser (Ctrl+click over a program that takes the mouse,
such as vim). Over SSH, the link is copied to your clipboard instead.
