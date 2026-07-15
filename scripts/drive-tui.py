#!/usr/bin/env python3
"""Drive leaf-tui in a real pty and print what it actually paints.

A development tool, not a test: it talks to the terminal leaf really runs on, so
it covers the one seam the unit tests can't reach — the event loop, the escape
sequences, and the grid as drawn. Use it to *see* a rendering or motion bug, then
pin the behaviour down in `leaf-core` where it can be asserted properly. It leans
on sleeps to let frames land, which is fine at a prompt and much too flaky for CI.

    cargo build
    scripts/drive-tui.py sample.md right'*'40
    COLS_=44 scripts/drive-tui.py sample.md wheeldown'*'4

Keys are named in SEQ below; `name*N` repeats one. Anything else is typed as
text. Note the quoting: `*` is the shell's, so it needs escaping.

Two traps, both of which produced convincing nonsense before being fixed, and
both of which look like leaf bugs rather than harness bugs:

  * Keys sent before leaf reaches raw mode are handed to it by the line
    discipline one byte at a time, so `\\x1b[C` arrives as Esc, `[`, `C` — and
    leaf types `[C` into your document. Hence the wait for a first frame.
  * An unreaped child from the last run steals enough CPU to cause exactly that
    race in the next one, so the failure comes and goes. Hence the waitpid.
"""
import fcntl, os, pty, select, struct, sys, termios, time

FILE = sys.argv[1]
COLS, ROWS = int(os.environ.get("COLS_", 62)), int(os.environ.get("ROWS_", 14))
BIN = os.environ.get("LEAF_", "./target/debug/leaf")

SEQ = {
    "right": "\x1b[C", "left": "\x1b[D", "down": "\x1b[B", "up": "\x1b[A",
    "home": "\x1b[H", "end": "\x1b[F", "enter": "\r", "tab": "\t",
    "backspace": "\x7f", "esc": "\x1b",
    # SGR mouse, at row 5 col 10: 64 is the wheel up, 65 down, 0 a left click.
    "wheelup": "\x1b[<64;10;5M", "wheeldown": "\x1b[<65;10;5M",
    "click": "\x1b[<0;10;5M",
    "quit": "\x11",
}

keys = []
for arg in sys.argv[2:]:
    name, _, count = arg.partition("*")
    keys += [name] * int(count or 1)

pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp(BIN, ["leaf", FILE])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))


def drain(seconds):
    out, end = b"", time.time() + seconds
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    return out


painted = b""
for _ in range(40):
    painted += drain(0.25)
    if b"leaf" in painted:
        break
painted += drain(0.4)
assert b"leaf" in painted, "leaf never painted a first frame"

for k in keys:
    assert k in SEQ or len(k) == 1, f"unknown key {k!r} — it would be typed in as text"
    os.write(fd, SEQ.get(k, k).encode())
    time.sleep(0.06)
    painted += drain(0.12)
os.write(fd, SEQ["quit"].encode())
time.sleep(0.2)
os.close(fd)
try:
    os.kill(pid, 9)
except ProcessLookupError:
    pass
os.waitpid(pid, 0)

# Replay the whole session onto a virtual screen: ratatui only repaints what
# changed, so the final frame is only legible on top of the ones before it.
screen = [[" "] * COLS for _ in range(ROWS)]
row = col = i = 0
text = painted.decode("utf-8", "replace")
while i < len(text):
    ch = text[i]
    if ch == "\x1b" and text[i + 1 : i + 2] == "[":
        j = i + 2
        while j < len(text) and text[j] not in "@ABCDEFGHJKSTfhlmnrsu":
            j += 1
        params, cmd = text[i + 2 : j], text[j : j + 1]
        if cmd == "H":
            p = [int(x) if x else 1 for x in params.split(";")] + [1, 1]
            row, col = p[0] - 1, p[1] - 1
        elif cmd == "J":
            screen = [[" "] * COLS for _ in range(ROWS)]
        i = j + 1
        continue
    if ch == "\n":
        row, col = row + 1, 0
    elif ch == "\r":
        col = 0
    elif ch >= " ":
        if 0 <= row < ROWS and 0 <= col < COLS:
            screen[row][col] = ch
        col += 1
    i += 1

for n, line in enumerate(screen):
    print(f"{n:2} |{''.join(line).rstrip()}")
