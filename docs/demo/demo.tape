# The terminal half of the demo, for vhs (https://github.com/charmbracelet/vhs).
# Run docs/demo/setup.sh first, then, from the repo root: vhs docs/demo/demo.tape
#
# The timings are guesses: how long Claude takes to reach its permission prompt
# depends on the day. Adjust the Sleeps after a dry run.

Output docs/demo/terminal.mp4
Output docs/demo/terminal.gif

Set Shell bash
Set FontSize 18
Set Width 1400
Set Height 820
Set Padding 16
Set TypingSpeed 60ms
Set Framerate 24

Hide
Type "source /tmp/valk-demo/env && clear" Enter
Show

# 1. Several sessions, one home screen.
Type "valk" Sleep 400ms Enter
Sleep 4s

# 2. "fix login" flips to needs input: it rises to the top, chimes, and the
#    summary says what Claude asked. (The chime isn't recorded; vhs has no audio.)
Sleep 12s

# 3-4. The phone answers the prompt (recorded separately); here the session
#      leaves the queue. Wait for it, and for the agent to get back to work.
Sleep 10s
