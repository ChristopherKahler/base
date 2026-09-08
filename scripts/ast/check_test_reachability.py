#!/usr/bin/env python3
"""Every test in scripts/ast/ must actually RUN under the CI runner.

Called from the `python-ast-tests` job in `.github/workflows/ci.yml`. NOT named
`test_*.py` on purpose: it must not be picked up by that job's own glob.

THE TRAP THIS CLOSES
--------------------
`ci.yml` runs each discovered file as `python "$t"`, not under pytest -- and
pytest is not installed in that job (`scripts/ast/requirements.txt` carries
tree-sitter, PyYAML and the grammars; zero occurrences of pytest). So a test
function executes only if something reachable from that file's
`if __name__ == "__main__":` block calls it.

The job's existing guard counts FILES: "found N test file(s)", and zero is a
failure. It does not count TESTS. A new file with three `def test_*` and no
driver is discovered, executed, prints nothing, asserts nothing, exits 0, and
the job goes green while the guard cheerfully reports it found the file.

Measured side by side at `main = 29319c8c`: the shipped step on a bare
pytest-style file exits **0** ("found 1 test file(s) ... rc=0"); this checker on
the same file exits **1** naming both tests; and the same shipped step on a file
whose test IS called and fails exits **1** -- so that 0 means "nothing to fail",
not "cannot fail".

WHAT COUNTS AS REACHABLE
------------------------
Reachability is a TRANSITIVE CLOSURE over module-level functions, seeded from
the `__main__` block, not a check that `__main__` names each test directly. One
indirection defeated the direct-name version: a `__main__` that calls a local
`run_all()` which calls every test reported every test UNREACHABLE while the
tests ran perfectly. Nothing is allow-listed by name to fix that -- `run_all`
appears nowhere below -- because the next file will call it something else.

  A  tests defined, every one reachable                              -> OK
  B  tests defined, at least one NOT reachable                       -> FAIL
  C  no `def test_*`, but `__main__` drives something (e.g. main())   -> OK
  D  no `def test_*` and no `__main__` at all                        -> FAIL
     (running the file asserts nothing)
  I  tests defined, none unreachable, at least one INDETERMINATE     -> OK
  U  the file could not be PARSED at all                            -> FAIL
     (its own bucket: never counted as unreachable, and the walk
      continues so the rest of the directory is still reported)

A `__main__` block that sweeps `globals()`/`vars()` for test names, or delegates
to `pytest.main()`/`unittest.main()`, reaches every module-level test without
naming any; that is detected anywhere in the closure and treated as a driver
rather than reported as false unreachables. It does NOT excuse a nested test:
a `globals()` sweep cannot see a function defined inside another function.

NESTED `def test_*`
-------------------
A test defined inside another function is classified by whether its enclosing
function is reachable, and by how that function uses it:

  * enclosing function NOT reachable            -> UNREACHABLE (it cannot run)
  * enclosing reachable and CALLS it by name    -> reachable
  * enclosing reachable and only RETURNS or
    otherwise mentions it                       -> INDETERMINATE

INDETERMINATE is reported on its own line and counted in the totals; it does
NOT fail the build. `f = build()` followed by `f()` somewhere else is not
decidable from the syntax tree, and a file in exactly that shape -- condor's
armH, pinned below at its md5 -- genuinely DOES run its nested test. Failing it
would make this checker assert that a running test never runs, which is the
defect it exists to catch, wearing the other hat.

KNOWN AND DELIBERATE LIMITATION -- PRE-REGISTERED
-------------------------------------------------
A test name that merely APPEARS inside a reachable function, without ever being
called, reads as reachable. `registry = [test_alpha]` with nothing iterating
`registry` is a false PASS, and `--selftest` arm P measures exactly that rather
than leaving it to be discovered.

This is a deliberate trade, and it is the same trade as INDETERMINATE above:
**prefer a narrow MISS over a false FAIL.** Counting only direct calls would
instead red a legitimate shape -- `for fn in (test_alpha, test_beta): fn()`,
where the tests are `ast.Name` loads and never `ast.Call` funcs (arm L). A gate
that reds correct files gets switched off; a gate with a named narrow blind spot
does not. A miss fails to detect a defect; a false FAIL ships a claim that is
untrue, and only the second breaks the bar this repo is held to.

This checker also does not verify that a reachable test ASSERTS anything --
`def test_x(): pass` called from `__main__` is shape A and passes. `sys.settrace`
proves a function executed, not that it asserted; that is a different guard.

usage:  check_test_reachability.py [dir ...]     (default: this file's directory)
        check_test_reachability.py --selftest    (control arms; run this first)

exit:   0  every test file asserts something when run as `python <file>`
        1  at least one file cannot assert anything, has unreachable tests,
           or could not be parsed at all
        2  VOID -- visited zero files, so this run proved NOTHING

Law 23: prints the size of every set it visits; a zero visit is rc 2, never a pass.
Law 24: `--selftest` is the control arm. Every arm MUST land on a specific rc, and
        several also assert the exact NAMES reported -- a gate whose failing and
        non-running states share an exit code is not a gate, and neither is one
        that lands on the right rc while naming the wrong test.
Law 27: rc is ASSIGNED into a variable before any reporting and the process ends
        on an explicit sys.exit(rc). The last statement is never a grep or a print.
"""
import ast
import contextlib
import hashlib
import io
import os
import sys
import tempfile

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except (AttributeError, ValueError):
    pass

_SWEEPERS = ("globals", "locals", "vars")
_DELEGATES = ("main", "cmdline", "run_module")


def _refs_in(node, skip_nested_defs=True):
    """Every identifier `node` references, plus sweeper/delegate flags.

    Collects Call targets (`ast.Name.id`, `ast.Attribute.attr`) AND bare
    `ast.Name` loads, because a driver may reach a test without ever making it
    the func of a Call: `for fn in (test_alpha, test_beta): fn()`.

    Nested `def` bodies are skipped by default: a function defined inside
    `node` is a separate scope with its own reachability, and folding its body
    in here would make every nested test look reached by its parent.
    """
    named, sweeps, delegates = set(), False, False
    stack = [(node, True)]
    while stack:
        cur, is_root = stack.pop()
        for child in ast.iter_child_nodes(cur):
            if (skip_nested_defs
                    and isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef))):
                # its NAME is a reference; its BODY is another scope
                named.add(child.name)
                continue
            stack.append((child, False))
        if isinstance(cur, ast.Call):
            f = cur.func
            if isinstance(f, ast.Name):
                named.add(f.id)
                if f.id in _SWEEPERS:
                    sweeps = True
                if f.id in _DELEGATES:
                    delegates = True
            elif isinstance(f, ast.Attribute):
                named.add(f.attr)
                if f.attr in _DELEGATES:
                    delegates = True
        elif isinstance(cur, ast.Name) and isinstance(cur.ctx, ast.Load):
            named.add(cur.id)
        elif isinstance(cur, ast.Attribute) and isinstance(cur.ctx, ast.Load):
            named.add(cur.attr)
    return named, sweeps, delegates


def _calls_in(node):
    """Only the identifiers `node` actually CALLS. Used to separate a nested
    test its parent invokes from one the parent merely returns."""
    called = set()
    stack = [node]
    while stack:
        cur = stack.pop()
        for child in ast.iter_child_nodes(cur):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            stack.append(child)
        if isinstance(cur, ast.Call):
            f = cur.func
            if isinstance(f, ast.Name):
                called.add(f.id)
            elif isinstance(f, ast.Attribute):
                called.add(f.attr)
    return called


def _module_functions(tree):
    """name -> node for everything the closure can step through: module-level
    functions, and methods of `Test*` classes keyed by their bare method name
    (an attribute call records only the attr)."""
    funcs = {}
    for n in tree.body:
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            funcs[n.name] = n
        elif isinstance(n, ast.ClassDef):
            for m in n.body:
                if isinstance(m, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    funcs.setdefault(m.name, m)
    return funcs


def _collect_tests(tree):
    """Every `test*` function, with how it is scoped.

    Returns a list of dicts: {name, qual, kind, enclosing}
      kind "module"  -- top-level def
      kind "class"   -- method of a `Test*` class
      kind "nested"  -- defined inside another function
    """
    found = []
    for n in tree.body:
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name.startswith("test"):
            found.append({"name": n.name, "qual": n.name,
                          "kind": "module", "enclosing": None})
        elif isinstance(n, ast.ClassDef) and n.name.startswith("Test"):
            for m in n.body:
                if isinstance(m, (ast.FunctionDef, ast.AsyncFunctionDef)) and m.name.startswith("test"):
                    found.append({"name": m.name, "qual": "%s.%s" % (n.name, m.name),
                                  "kind": "class", "enclosing": None})

    # nested: any test* def whose nearest function ancestor is a function
    for parent in ast.walk(tree):
        if not isinstance(parent, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for child in ast.walk(parent):
            if child is parent:
                continue
            if not isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if not child.name.startswith("test"):
                continue
            if _nearest_function_parent(tree, child) is not parent:
                continue
            found.append({"name": child.name,
                          "qual": "%s.<local>.%s" % (parent.name, child.name),
                          "kind": "nested", "enclosing": parent.name})
    return found


def _nearest_function_parent(tree, target):
    """The innermost FunctionDef that directly encloses `target`, or None."""
    best = None
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for child in ast.walk(node):
            if child is target and child is not node:
                # `node` encloses target; keep the deepest such node
                if best is None or _depth_of(node, best):
                    best = node
                break
    return best


def _depth_of(candidate, current):
    """True when `candidate` is nested inside `current` (so it is deeper)."""
    for child in ast.walk(current):
        if child is candidate and child is not current:
            return True
    return False


def _reachable_closure(tree, main_blocks):
    """Names reachable from `__main__`, to a fixpoint, plus the generic flag.

    Seeded with everything the `__main__` blocks reference; then any seeded name
    that is a module-level function contributes its own references, repeatedly,
    until the set stops growing. Finite name set, monotone growth: it terminates.
    """
    funcs = _module_functions(tree)
    reached, sweeps, delegates = set(), False, False
    for b in main_blocks:
        r, s, d = _refs_in(b)
        reached |= r
        sweeps = sweeps or s
        delegates = delegates or d

    seen = set()
    while True:
        frontier = [n for n in reached if n in funcs and n not in seen]
        if not frontier:
            break
        for name in frontier:
            seen.add(name)
            r, s, d = _refs_in(funcs[name])
            reached |= r
            sweeps = sweeps or s
            delegates = delegates or d
    return reached, (sweeps or delegates), funcs


def audit_file(path):
    with open(path, "rb") as fh:
        tree = ast.parse(fh.read().decode("utf-8"))

    tests = _collect_tests(tree)

    main_blocks = [
        n for n in tree.body
        if isinstance(n, ast.If)
        and isinstance(n.test, ast.Compare)
        and isinstance(n.test.left, ast.Name)
        and n.test.left.id == "__name__"
    ]

    reached, generic, funcs = _reachable_closure(tree, main_blocks)
    driver = bool(main_blocks) and (bool(reached) or generic)

    unreachable, indeterminate = [], []
    for t in tests:
        if t["kind"] == "nested":
            # a globals()/vars() sweep cannot see a function defined inside
            # another function, so `generic` does not excuse a nested test.
            enclosing = t["enclosing"]
            if enclosing not in reached:
                unreachable.append(t["qual"])
            elif t["name"] in _calls_in(funcs.get(enclosing) or ast.Module(body=[], type_ignores=[])):
                pass  # the parent calls it by name
            else:
                indeterminate.append(t["qual"])
        else:
            if generic:
                continue
            if t["name"] not in reached and t["qual"] not in reached:
                unreachable.append(t["qual"])

    defined = [t["qual"] for t in tests]
    if defined:
        if unreachable:
            shape = "B"
        elif indeterminate:
            shape = "I"
        else:
            shape = "A"
    else:
        shape = "C" if driver else "D"
    return defined, unreachable, indeterminate, shape, bool(main_blocks), generic


_SHAPE_NOTE = {
    "A": "def test_* + every one reachable from __main__",
    "B": "def test_* NOT all reachable from __main__",
    "C": "no def test_*, __main__ drives (main()/sweeper/delegate)",
    "D": "no def test_* and no __main__ - running this file asserts NOTHING",
    "I": "def test_* all reachable or INDETERMINATE, none proven unreachable",
    "U": "UNPARSEABLE - could not be read at all, so nothing about it is known",
}


def audit_dirs(dirs):
    files = []
    for d in dirs:
        if not os.path.isdir(d):
            print("VOID: not a directory: %s" % d)
            continue
        for name in sorted(os.listdir(d)):
            if name.startswith("test_") and name.endswith(".py"):
                files.append(os.path.join(d, name))

    print("visited_dirs=%d visited_files=%d" % (len(dirs), len(files)))
    if not files:
        print("VOID: zero test_*.py visited - this run proved NOTHING")
        return 2

    bad = 0
    total_defined = total_unreach = total_indet = total_unparse = 0
    for p in files:
        # A file that could not be READ and a file whose tests are DEAD are the
        # two states law 24 exists to separate, so an unparseable file gets its
        # own bucket and is never counted as unreachable. It also must not stop
        # the walk: aborting here would print no totals at all and say nothing
        # about the files after it, which would make this module's own law-23
        # claim false and quietly degrade the gate to a syntax check.
        try:
            defined, unreachable, indeterminate, shape, has_main, generic = audit_file(p)
        except (SyntaxError, UnicodeDecodeError) as exc:
            total_unparse += 1
            bad += 1
            print("%s %-32s shape=U %s" % ("!!", os.path.basename(p), _SHAPE_NOTE["U"]))
            print("     UNPARSEABLE, not unreachable: %s: %s" % (type(exc).__name__, exc))
            continue
        total_defined += len(defined)
        total_unreach += len(unreachable)
        total_indet += len(indeterminate)
        # An INDETERMINATE never masks a FAIL: one unreachable test fails the
        # file however many INDETERMINATEs sit beside it.
        ok = not unreachable and shape in ("A", "C", "I")
        bad += 0 if ok else 1
        print("%s %-32s shape=%s defined=%d unreachable=%d indeterminate=%d __main__=%s generic=%s  (%s)"
              % ("OK" if ok else "!!", os.path.basename(p), shape, len(defined),
                 len(unreachable), len(indeterminate), has_main, generic,
                 _SHAPE_NOTE[shape]))
        for name in unreachable:
            print("     UNREACHABLE under `python <file>`: %s" % name)
        for name in indeterminate:
            print("     INDETERMINATE (reachable enclosing returns it rather than "
                  "calling it; cannot be decided statically): %s" % name)
        if shape == "D":
            print("     add a `if __name__ == \"__main__\":` block that calls its assertions")

    print("totals: files=%d defined_tests=%d reachable=%d unreachable=%d "
          "indeterminate=%d unparseable_files=%d failing_files=%d  (VOID is a "
          "whole-run state: rc 2 when zero files are visited)"
          % (len(files), total_defined, total_defined - total_unreach - total_indet,
             total_unreach, total_indet, total_unparse, bad))
    return 1 if bad else 0


# --------------------------------------------------------------- control arms

_ARM_A = 'def test_alpha():\n    assert True\n\n\nif __name__ == "__main__":\n    test_alpha()\n'
_ARM_B = 'def test_alpha():\n    assert True\n\n\ndef test_beta():\n    assert True\n'
_ARM_C = ('import sys\n\n\ndef main() -> int:\n    assert True\n    return 0\n\n\n'
          'if __name__ == "__main__":\n    sys.exit(main())\n')
_ARM_D = 'X = 1\n'

# F/G: the indirect-driver shapes. F is condor's armF verbatim in shape; G adds
# a second hop, because a fix that resolves ONE indirection still fails G.
_ARM_F = ('def test_alpha():\n    assert True\n\n\ndef test_beta():\n    assert True\n\n\n'
          'def drive():\n    test_alpha()\n    test_beta()\n\n\n'
          'if __name__ == "__main__":\n    drive()\n')
_ARM_G = ('def test_alpha():\n    assert True\n\n\ndef inner():\n    test_alpha()\n\n\n'
          'def outer():\n    inner()\n\n\n'
          'if __name__ == "__main__":\n    outer()\n')

# L: loop dispatch. The benefit the bare-Name widening buys, measured.
_ARM_L = ('def test_alpha():\n    assert True\n\n\ndef test_beta():\n    assert True\n\n\n'
          'if __name__ == "__main__":\n    for fn in (test_alpha, test_beta):\n        fn()\n')

# K: half reachable. Lands on rc 1 before AND after the fix, so only the NAMES
# separate a fixed checker from a broken one.
_ARM_K = ('def test_alpha():\n    assert True\n\n\ndef test_beta():\n    assert True\n\n\n'
          'def drive():\n    test_alpha()\n\n\n'
          'if __name__ == "__main__":\n    drive()\n')

# J: defined, and the driver never reaches it. Must still FAIL after the fix.
_ARM_J = ('def test_alpha():\n    assert True\n\n\ndef drive():\n    test_alpha()\n\n\n'
          'if __name__ == "__main__":\n    print("driver does not call drive")\n')

# M/N: nested, reached and not reached.
_ARM_M = ('def outer():\n    def test_inner():\n        assert True\n\n    test_inner()\n\n\n'
          'if __name__ == "__main__":\n    outer()\n')
_ARM_N = ('import sys\n\n\ndef _factory():\n    def test_never_runs():\n'
          '        raise AssertionError("must never be counted as reachable")\n\n'
          '    return test_never_runs\n\n\ndef main() -> int:\n    return 0\n\n\n'
          'if __name__ == "__main__":\n    sys.exit(main())\n')

# X: class-method dispatch. Passed on the OLD checker only via the second
# disjunct of a split-and-compare -- load-bearing by accident. R3 pins it.
_ARM_X = ('class TestThing:\n    def test_alpha(self):\n        assert True\n\n\n'
          'if __name__ == "__main__":\n    TestThing().test_alpha()\n')

# P: the PRE-REGISTERED false PASS. test_alpha is mentioned inside a reachable
# function and never called, and reads reachable. Measured, not merely admitted.
_ARM_P = ('def test_alpha():\n    assert True\n\n\ndef drive():\n'
          '    registry = [test_alpha]\n    print(len(registry))\n\n\n'
          'if __name__ == "__main__":\n    drive()\n')

# W: an INDETERMINATE must never mask a FAIL sitting in the same file.
_ARM_W = ('def build():\n    def test_returned():\n        assert True\n'
          '    return test_returned\n\n\ndef test_orphan():\n    assert True\n\n\n'
          'if __name__ == "__main__":\n    build()\n')

# H: condor's armH, BYTE-IDENTICAL and pinned by md5. The counterexample that
# decided INDETERMINATE must not fail: `build()()` genuinely RUNS test_inner,
# so rc 1 here would be this checker asserting that a running test never runs.
_ARM_H = ('def build():\n    def test_inner():\n        assert True\n'
          '    return test_inner\n\n\n'
          'if __name__ == "__main__":\n    build()()\n')
_ARM_H_MD5 = "d7b0e2825c61705aca0d3f59a6017b86"


def _arm(label, body, want_rc, want_unreachable=None, want_indeterminate=None):
    """Run one control arm. Asserts rc, and where given the exact NAME sets --
    arm K lands on rc 1 whether or not the fix works, so rc alone cannot grade
    it."""
    with tempfile.TemporaryDirectory(prefix="reach-arm-") as d:
        with open(os.path.join(d, "test_arm.py"), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(body)
        print("--- control arm %s (expect rc %d)" % (label, want_rc))
        path = os.path.join(d, "test_arm.py")
        rc = audit_dirs([d])
        fails = 0
        if rc != want_rc:
            print("    ARM FAIL: rc=%d, expected %d" % (rc, want_rc))
            fails += 1
        if want_unreachable is not None or want_indeterminate is not None:
            _defined, unreach, indet, _shape, _hm, _g = audit_file(path)
            got_u = set(n.split(".")[-1] for n in unreach)
            got_i = set(n.split(".")[-1] for n in indet)
            if want_unreachable is not None and got_u != set(want_unreachable):
                print("    ARM FAIL: unreachable=%s, expected %s"
                      % (sorted(got_u), sorted(want_unreachable)))
                fails += 1
            if want_indeterminate is not None and got_i != set(want_indeterminate):
                print("    ARM FAIL: indeterminate=%s, expected %s"
                      % (sorted(got_i), sorted(want_indeterminate)))
                fails += 1
        print("arm %s rc=%d" % (label, rc))
        return fails


def _arm_unparseable():
    """Law 23 arm: ONE unparseable file must not stop the walk.

    Three clauses, and the third is the one that gets dropped:
      1. rc is 1 -- the run happened, the gate cannot certify the tree
      2. the broken file is NAMED, and named as UNPARSEABLE rather than as
         unreachable: a file that could not be read and a file whose tests are
         dead are the two states law 24 exists to separate
      3. BOTH good files are audited and counted

    The second good file sorts AFTER the broken one on purpose. Without that,
    an arm passes while proving only that the crash moved.
    """
    files = {
        "test_aaa_good.py": 'def test_a():\n    assert True\n\n\nif __name__ == "__main__":\n    test_a()\n',
        "test_bbb_broken.py": "def test_broken(:\n    this is not python\n",
        "test_zzz_good.py": 'def test_z():\n    assert True\n\n\nif __name__ == "__main__":\n    test_z()\n',
    }
    print("--- control arm U  one unparseable file among two good ones (expect rc 1)")
    with tempfile.TemporaryDirectory(prefix="reach-arm-unparse-") as d:
        for name, body in files.items():
            with open(os.path.join(d, name), "w", encoding="utf-8", newline="\n") as fh:
                fh.write(body)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = audit_dirs([d])
        out = buf.getvalue()
    print(out, end="")

    fails = 0
    if rc != 1:
        print("    ARM FAIL: rc=%d, expected 1" % rc)
        fails += 1
    if "test_bbb_broken.py" not in out or "UNPARSEABLE" not in out:
        print("    ARM FAIL: the unparseable file was not named as UNPARSEABLE")
        fails += 1
    if "unreachable: test_broken" in out:
        print("    ARM FAIL: an unparseable file was reported as UNREACHABLE")
        fails += 1
    # clause 3: the walk CONTINUED past the break
    for good in ("test_aaa_good.py", "test_zzz_good.py"):
        if good not in out:
            print("    ARM FAIL: %s was never audited - the walk stopped" % good)
            fails += 1
    if "files=3 defined_tests=2" not in out:
        print("    ARM FAIL: totals did not count 3 files and 2 defined tests")
        fails += 1
    if "unparseable_files=1" not in out:
        print("    ARM FAIL: totals did not report unparseable_files=1")
        fails += 1
    print("arm U  one unparseable among two good rc=%d" % rc)
    return fails


def selftest():
    fails = 0
    fails += _arm("A  driver calls its test", _ARM_A, 0, [], [])
    fails += _arm("B  two tests, NO driver", _ARM_B, 1, ["test_alpha", "test_beta"], [])
    fails += _arm("C  main() + sys.exit(main())", _ARM_C, 0, [], [])
    fails += _arm("D  no tests and no __main__", _ARM_D, 1, [], [])
    fails += _arm("F  indirect driver, one hop", _ARM_F, 0, [], [])
    fails += _arm("G  indirect driver, two hops", _ARM_G, 0, [], [])
    fails += _arm("L  loop dispatch over a tuple", _ARM_L, 0, [], [])
    fails += _arm("K  driver reaches ONE of two", _ARM_K, 1, ["test_beta"], [])
    fails += _arm("J  driver never reaches the test", _ARM_J, 1, ["test_alpha"], [])
    fails += _arm("M  nested test, parent CALLS it", _ARM_M, 0, [], [])
    fails += _arm("N  nested test, parent unreachable", _ARM_N, 1, ["test_never_runs"], [])
    fails += _arm("X  class method via TestThing()", _ARM_X, 0, [], [])
    fails += _arm("P  PRE-REGISTERED false pass: named, never called", _ARM_P, 0, [], [])
    fails += _arm("W  INDETERMINATE must not mask a FAIL", _ARM_W, 1,
                  ["test_orphan"], ["test_returned"])

    fails += _arm_unparseable()

    # condor's armH, pinned. If the embedded bytes ever stop hashing to the
    # recorded md5 the arm fails rather than silently grading a different
    # fixture -- a control that quietly changes what it measures is not one.
    got = hashlib.md5(_ARM_H.encode("utf-8")).hexdigest()
    print("--- control arm H  condor armH, pinned md5 %s" % _ARM_H_MD5)
    if got != _ARM_H_MD5:
        print("    ARM FAIL: embedded armH hashes to %s, expected %s - the pinned "
              "counterexample has drifted" % (got, _ARM_H_MD5))
        fails += 1
    else:
        print("    md5 OK: %s" % got)
        fails += _arm("H  nested RETURNED then called: build()()", _ARM_H, 0, [], ["test_inner"])

    with tempfile.TemporaryDirectory(prefix="reach-arm-empty-") as d:
        print("--- control arm E  empty dir (expect rc 2 VOID)")
        rc = audit_dirs([d])
        print("arm E rc=%d" % rc)
        fails += 0 if rc == 2 else 1

    print("SELFTEST FAIL COUNT = %d" % fails)
    return fails


def main():
    args = sys.argv[1:]
    if args and args[0] == "--selftest":
        return selftest()
    dirs = args or [os.path.dirname(os.path.abspath(__file__))]
    return audit_dirs(dirs)


if __name__ == "__main__":
    sys.exit(main())
