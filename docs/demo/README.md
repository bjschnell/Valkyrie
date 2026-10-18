# The demo

## The GIF in the README

`docs/images/demo.gif` is drawn from the TUI itself, not recorded. The test
`demo_walks_a_prompt_through_the_queue` renders four home screens with made-up
sessions, and `frames.py` turns them into a looping GIF with a caption under each
frame. To change it, edit the test or the captions, then:

```sh
VALK_SNAPSHOT_DIR=/tmp/valk-frames cargo test -p valkyrie-tui --lib demo_walks
python3 docs/demo/frames.py /tmp/valk-frames docs/images/demo.gif
```

It needs rsvg-convert, ImageMagick and ffmpeg. It has no phone and no sound, so a
real recording can replace it later.

## Recording a real one

A real recording would also show the phone and the chime. Keep it under 30
seconds and 5 MB, and save it over `docs/images/demo.gif`. An MP4 uploaded
through GitHub's editor and embedded by its URL works too.

Use made-up sessions only. Keep anything from work out of it, including repo
names, branch names, prompts and file paths. `setup.sh` builds a fake repo
(`acme-shop`) and runs a separate daemon for it, so your real sessions stay off
screen.

### Storyboard

| # | Seconds | On screen |
|---|---|---|
| 1 | 0-5 | `valk` home screen with three sessions running: `fix login`, `docs pass`, `dev server`. |
| 2 | 5-12 | `fix login` flips to **needs input** and moves to the top of the queue. The chime plays, and the summary shows what Claude asked ("Run npm test?"). |
| 3 | 12-22 | Cut to the phone: open the Valkyrie app, the prompt's answers are buttons in the inbox, tap **Yes**. |
| 4 | 22-28 | Back on the terminal: `fix login` leaves the queue and goes back to work. Loop. |

### Setup

```sh
docs/demo/setup.sh            # demo daemon, fake repo, three sessions
source /tmp/valk-demo/env     # this shell now talks to the demo daemon
valk                          # check it looks right
```

Claude asks whether it trusts a folder it hasn't seen before. Accept that for
`/tmp/valk-demo/acme-shop` before recording, or the first thing to need input is
the trust dialog instead of the test run. One way is to run
`cd /tmp/valk-demo/acme-shop && claude` once, accept, then quit.

When you're done: `docs/demo/setup.sh --stop`.

### Terminal part

Use vhs or asciinema. vhs is scripted, so every take comes out the same:

```sh
vhs docs/demo/demo.tape       # writes docs/demo/terminal.mp4 and terminal.gif
```

With asciinema, you record by hand and convert the cast with agg:

```sh
asciinema rec /tmp/valk-demo/demo.cast -c valk
agg --theme dracula --font-size 18 /tmp/valk-demo/demo.cast docs/demo/terminal.gif
```

Neither records sound. If you want the chime in the MP4, record the terminal with
OBS or your screen recorder instead.

### Phone part

The phone has to talk to the demo daemon, not your real one. In the shell where
you sourced `/tmp/valk-demo/env`:

```sh
valk web --listen 127.0.0.1:8791 &   # 8790 is taken if `valk setup web` runs
tailscale serve --bg --https 8443 http://127.0.0.1:8791
valk web pair                        # scan it, or type the short code
```

Then screen-record the phone (iOS: Control Center → Screen Recording; Android:
Quick Settings → Screen record) while you tap the answer to the `fix login`
prompt. Afterwards, `tailscale serve --https=8443 off`.

### Putting it together

Cut the terminal and phone clips together (any editor; `ffmpeg` works), then make
the GIF small:

```sh
# Scale to 960 px wide, 12 fps, with a palette made for this clip.
ffmpeg -i demo.mp4 -vf "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse" docs/images/demo.gif
ls -lh docs/images/demo.gif   # must be under 5 MB; lower fps or width if not
```

The README already shows `docs/images/demo.gif`, so replacing the file is enough.
