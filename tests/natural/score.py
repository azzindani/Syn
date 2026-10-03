#!/usr/bin/env python3
"""Score one natural session against the machine-checkable gates of
EXPECTATIONS.md (A1-A3, C1-C3, D2-D3). The rest is read by a person.

    python tests/natural/score.py <session.log> [--sources DIR] [--script FILE]

<session.log> is what drive.ps1 wrote; the per-turn files it saved beside it
(<name>-turn<N>.txt) give the steps and refusals of each turn. Prints a
report and exits 1 if a machine-checked gate fails.
"""
import argparse
import hashlib
import os
import re
import statistics
import sys

TURN = re.compile(r"\[(\d\d:\d\d:\d\d)\] TURN (\d+) DONE in (\d+)s, (\d+) step\(s\): (\w+) ?(.*)")
SENT = re.compile(r"\[(\d\d:\d\d:\d\d)\] TURN (\d+) SENT")


def read(path):
    return open(path, encoding="utf-8-sig", errors="replace").read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("log")
    ap.add_argument("--script", help="the tests/natural/*.md played, to know how many turns to expect")
    ap.add_argument("--expect", type=int, default=0, help="turns expected (default: counted from --script, else 50)")
    args = ap.parse_args()

    log = read(args.log)
    turns = {}
    for m in TURN.finditer(log):
        _, n, secs, steps, kind, text = m.groups()
        turns[int(n)] = dict(secs=int(secs), steps=int(steps), kind=kind, text=text)
    sent = {int(m.group(2)) for m in SENT.finditer(log)}
    expect = args.expect
    if not expect and args.script:
        expect = len(re.findall(r"^\d+\. ", read(args.script), re.M))
    expect = expect or 50

    base = re.sub(r"\.log$", "", args.log)
    refused = calls = repeated = 0
    per_turn_refused = {}
    for n in turns:
        f = f"{base}-turn{n}.txt"
        if not os.path.exists(f):
            continue
        body = read(f)
        calls += len(re.findall(r"^RECEIPT step ", body, re.M))
        r = len(re.findall(r"^REFUSED ", body, re.M))
        refused += r
        per_turn_refused[n] = r
        repeated += body.count("this is the same call as the two before")

    secs = [t["secs"] for t in turns.values()]
    gates = []

    def gate(name, ok, detail):
        gates.append((name, ok, detail))

    missing = [n for n in range(1, expect + 1) if n not in turns]
    bad = {n: t for n, t in turns.items() if t["kind"] != "ANSWER"}
    gate("A1 every turn answers", not missing and not bad,
         f"{len(turns)}/{expect} turns done; not ANSWER: {[(n, t['kind'], t['text'][:60]) for n, t in bad.items()]}; missing: {missing}")
    rescued = sorted(n for n, t in turns.items() if t["secs"] > 1200)
    gate("A2 nobody intervened (no turn over 20 min)", not rescued, f"turns over 20 min: {rescued}")
    over = sorted(n for n, t in turns.items() if t["steps"] >= 100 or "Step budget ran out" in t["text"])
    gate("A3 no step budget used up", not over, f"turns at 100 steps: {over}")
    if secs:
        med = statistics.median(secs)
        p90 = sorted(secs)[max(0, int(len(secs) * 0.9) - 1)]
        gate("C1 median <= 60 s, p90 <= 240 s", med <= 60 and p90 <= 240, f"median {med:.0f} s, p90 {p90} s")
        heavy = sorted(n for n, t in turns.items() if t["steps"] >= 60)
        gate("C2 at most 3 turns of 60+ steps", len(heavy) <= 3, f"{len(heavy)} turns: {heavy}")
        total = sum(secs) / 60
        gate("C3 whole script <= 90 min", total <= 90, f"{total:.0f} min of turn time")
    rate = (refused / calls) if calls else 0
    gate("D2 refused calls <= 8%", rate <= 0.08, f"{refused} of {calls} calls ({rate:.1%}); worst turns: "
         + str(sorted(per_turn_refused.items(), key=lambda kv: -kv[1])[:5]))
    gate("D3 the repeated-call gate never fired", repeated == 0, f"{repeated} times")

    print(f"# {os.path.basename(args.log)}: {len(turns)} turns, {sum(secs) / 60:.0f} min, {sum(t['steps'] for t in turns.values())} steps")
    for name, ok, detail in gates:
        print(f"{'PASS' if ok else 'FAIL'}  {name}: {detail}")
    slow = sorted(((t["secs"], n, t["steps"]) for n, t in turns.items()), reverse=True)[:6]
    print("slowest turns (secs, turn, steps):", slow)
    print("B, D1, D4, E, F, G are read by a person: see tests/natural/EXPECTATIONS.md")
    return 0 if all(ok for _, ok, _ in gates) else 1


if __name__ == "__main__":
    sys.exit(main())
