#!/usr/bin/env python3
"""PTY probe: run a command in a pty, send keys, print the screen after each key.

    python3 keyprobe.py <cols> <rows> '<keys>' <command...>

Key tokens: <up> <down> <left> <right> <tab> <pgup> <pgdn> <home> <end> <enter>
<esc> <space>, plus literal single characters. `cc` is two `c` presses. Wrap the
binary, never `make` — wrapping make hangs the pty and the capture comes out
empty.

The screen model is deliberately small: cells, cursor, clear, CR/LF. Colours and
attributes are dropped, because the check is the text ratatui draws. Reads arrive
in chunks that split escape sequences, so a partial sequence is kept in `pending`
and joined to the next chunk.
"""
import fcntl, os, pty, select, struct, sys, termios, time, unicodedata


def width(ch):
    """One cell for most characters, two for East Asian wide/fullwidth."""
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


KEYS = {
    "<up>": b"\x1b[A", "<down>": b"\x1b[B", "<left>": b"\x1b[D", "<right>": b"\x1b[C",
    "<tab>": b"\t", "<pgup>": b"\x1b[5~", "<pgdn>": b"\x1b[6~", "<home>": b"\x1b[H",
    "<end>": b"\x1b[F", "<enter>": b"\r", "<esc>": b"\x1b", "<space>": b" ",
}


class Screen:
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.grid = [[" "] * w for _ in range(h)]
        self.x = self.y = 0
        self.pending = b""

    def put(self, ch):
        if self.x >= self.w:
            self.x = self.w - 1
        if 0 <= self.y < self.h and 0 <= self.x < self.w:
            self.grid[self.y][self.x] = ch

    def clear(self):
        self.grid = [[" "] * self.w for _ in range(self.h)]

    def feed(self, data):
        data = self.pending + data
        self.pending = b""
        i, n = 0, len(data)
        while i < n:
            b = data[i]
            if b == 0x1B:
                if i + 1 >= n:  # escape at the end of a chunk
                    self.pending = data[i:]
                    return
                if data[i + 1] == 0x5B:  # '['
                    j = i + 2
                    params = ""
                    while j < n and not (0x40 <= data[j] <= 0x7E):
                        params += chr(data[j])
                        j += 1
                    if j >= n:  # sequence split across chunks
                        self.pending = data[i:]
                        return
                    self.seq(params, chr(data[j]))
                    i = j + 1
                    continue
                if data[i + 1] == 0x5D:  # ']': OSC, ignore to BEL
                    j = data.find(b"\x07", i)
                    if j < 0:
                        self.pending = data[i:]
                        return
                    i = j + 1
                    continue
                i += 2
                continue
            if b >= 0x80:  # UTF-8 sequence; skipping it without moving the cursor
                # shifts every later character left, which is what merged the rows
                ln = 4 if b >= 0xF0 else 3 if b >= 0xE0 else 2 if b >= 0xC0 else 1
                if i + ln > n:
                    self.pending = data[i:]
                    return
                self.put(data[i : i + ln].decode("utf-8", "replace"))
                self.x += width(data[i : i + ln].decode("utf-8", "replace"))
                i += ln
                continue
            if b == 0x0D:
                self.x = 0
            elif b == 0x0A:
                self.y += 1
            elif b == 0x08:
                self.x = max(0, self.x - 1)
            elif 0x20 <= b <= 0x7E:
                self.put(chr(b))
                self.x += 1
            i += 1

    def seq(self, params, final):
        p = params.lstrip("?")
        if final == "H":
            if p:
                parts = p.split(";")
                self.y = int(parts[0]) - 1 if parts[0] else 0
                self.x = int(parts[1]) - 1 if len(parts) > 1 and parts[1] else 0
            else:
                self.x = self.y = 0
        elif final == "K":
            for x in range(self.x, self.w):
                self.grid[self.y][x] = " "
        elif final == "J":
            if p in ("", "2"):
                self.clear()
        # SGR and cursor/screen-mode sequences carry nothing to capture.


def tokens(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == "<":
            j = s.find(">", i)
            if j > i and s[i : j + 1] in KEYS:
                out.append(KEYS[s[i : j + 1]])
                i = j + 1
                continue
        out.append(s[i].encode())
        i += 1
    return out


def main():
    w, h, seq, cmd = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4:]
    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(cmd[0], cmd)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", h, w, 0, 0))
    screen = Screen(w, h)

    def drain(timeout):
        end = time.time() + timeout
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    data = os.read(fd, 65536)
                except OSError:  # EIO once the child has exited
                    return False
                if not data:
                    return False  # EOF: the app quit
                screen.feed(data)
        return True

    def show(label):
        rows = ["".join(row).rstrip() for row in screen.grid]
        print(f"===== {label} =====", flush=True)
        for row in rows:
            if row.strip():
                print(row, flush=True)

    drain(1.0)
    show("start")
    for i, key in enumerate(tokens(seq), 1):
        os.write(fd, key)
        alive = drain(0.6)
        show(f"key {i}")
        if not alive:
            break
    os.close(fd)
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass


main()
