"""Rig relay core: sessions, file-handle registry, event stream. Stdlib only."""
import copy
import time


class HarnessError(Exception):
    pass


class DoomLoop(HarnessError):
    """Raised when last 3 ops are identical tool+args: requires human confirm."""


class Denied(HarnessError):
    pass


def new_handle(app, filename, unit):
    return "%s:%s:%s" % (app, filename, unit)


class Relay:
    def __init__(self):
        self.sessions = {}

    # -- sessions --
    def handshake(self, session_id, client="widget"):
        s = self.sessions.setdefault(session_id, {"files": {}, "events": [], "calls": [], "snapshots": {}})
        self.emit(session_id, {"t": "session.open", "session": session_id, "client": client})
        return {"session": session_id, "files": sorted(s["files"])}

    def ping(self):
        return {"status": "ok", "sessions": len(self.sessions)}

    # -- registry --
    def attach(self, session_id, handle, kind, content):
        s = self._s(session_id)
        s["files"][handle] = {"kind": kind, "content": content}
        s["snapshots"].setdefault(handle, [])
        self.emit(session_id, {"t": "file.attach", "handle": handle, "kind": kind})
        return {"handle": handle}

    def registry(self, session_id):
        return sorted(self._s(session_id)["files"])

    # -- events --
    def emit(self, session_id, event):
        event = dict(event)
        event["ts"] = time.time()
        self._s(session_id)["events"].append(event)
        return event

    def events(self, session_id):
        return list(self._s(session_id)["events"])

    # -- snapshots (per-handle undo scope) --
    def snapshot(self, session_id, handle):
        s = self._s(session_id)
        content = s["files"][handle]["content"]
        s["snapshots"][handle].append(copy.deepcopy(content))
        return len(s["snapshots"][handle])

    def undo(self, session_id, handle):
        s = self._s(session_id)
        stack = s["snapshots"][handle]
        if not stack:
            raise HarnessError("nothing to undo for %s" % handle)
        s["files"][handle]["content"] = stack.pop()
        self.emit(session_id, {"t": "step.undo", "handle": handle, "remaining": len(stack)})
        return {"handle": handle, "remaining": len(stack)}

    # -- doom-loop gate: last 3 identical (op, args) --
    def gate(self, session_id, op, args_key):
        calls = self._s(session_id)["calls"]
        calls.append((op, args_key))
        last3 = calls[-3:]
        if len(last3) == 3 and all(c == last3[0] for c in last3):
            raise DoomLoop("same op+args 3x (%s): human confirm required" % op)

    def _s(self, session_id):
        try:
            return self.sessions[session_id]
        except KeyError:
            raise HarnessError("unknown session %s: handshake first" % session_id)
