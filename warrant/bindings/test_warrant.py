"""The Python binding, against the real binary.

Run with the built binary on PATH, or point WARRANT_BIN at it:

    WARRANT_BIN=../target/release/warrant python3 -m unittest test_warrant -v
"""

import json
import os
import shutil
import tempfile
import unittest

from warrant import NeedsConfirmation, Refused, Warrant, WarrantError

BIN = os.environ.get("WARRANT_BIN", "warrant")

POLICY = """\
never   *        fs.read:/**/.ssh/**   # keys stay on the disk
deny    agent:*  pkg.install           # a person installs software
confirm agent:*  fs.delete:~/**        # say it out loud
allow   agent:*  fs.write:~/**
allow   agent:*  fs.read:~/**
"""

GRADES = """\
read     fs.read
write    fs.write
elevated fs.delete pkg.install
"""

AGENT = "agent:claude"


class Base(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="warrant-py-")
        with open(os.path.join(self.dir, "policy.warrant"), "w") as f:
            f.write(POLICY)
        with open(os.path.join(self.dir, "grades.warrant"), "w") as f:
            f.write(GRADES)
        self.w = Warrant(dir=self.dir, binary=BIN, home="/home/robert")

    def tearDown(self):
        self.w.close()
        shutil.rmtree(self.dir, ignore_errors=True)


class TestAsking(Base):
    def test_it_answers_with_the_line_that_decided(self):
        r = self.w.rule(AGENT, "fs.write:/home/robert/a.md")
        self.assertTrue(r.allowed)
        self.assertEqual(r.risk, "write")
        self.assertIn("policy.warrant:4", r.matched)

    def test_a_refusal_carries_its_reason(self):
        r = self.w.rule(AGENT, "pkg.install:htop")
        self.assertFalse(r.allowed)
        self.assertEqual(r.decision, "deny")
        self.assertEqual(r.reason, "a person installs software")

    def test_preflight_judges_a_whole_plan_and_records_nothing(self):
        rulings = self.w.preflight(
            AGENT,
            ["fs.read:/home/robert/a.md", "fs.write:/home/robert/b.md", "pkg.install:htop"],
        )
        self.assertEqual([r.allowed for r in rulings], [True, True, False])
        self.assertEqual(self.w.history(), [])


class TestActing(Base):
    def test_a_successful_action_is_recorded_whole(self):
        with self.w.act(
            AGENT,
            "fs.write:/home/robert/a.md",
            intent="save the draft",
            undo=("restore a.md", {"backup": "/var/b/1"}),
        ) as a:
            a.detail = "wrote 412 bytes"

        (rec,) = self.w.history()
        self.assertEqual(rec["outcome"], "ok")
        self.assertEqual(rec["intent"], "save the draft")
        self.assertEqual(rec["detail"], "wrote 412 bytes")
        self.assertEqual(rec["undo"]["data"]["backup"], "/var/b/1")

    def test_a_refused_action_raises_before_the_body_runs(self):
        ran = []
        with self.assertRaises(Refused):
            with self.w.act(AGENT, "pkg.install:htop"):
                ran.append(True)
        self.assertEqual(ran, [], "the body ran despite the refusal")

    def test_a_confirm_is_its_own_exception_not_a_plain_refusal(self):
        # "nobody asked" must never be mistaken for "somebody said yes".
        with self.assertRaises(NeedsConfirmation):
            with self.w.act(AGENT, "fs.delete:/home/robert/old.txt"):
                pass

        with self.w.act(
            AGENT, "fs.delete:/home/robert/old.txt", confirmed_by="robert"
        ):
            pass
        (rec,) = self.w.history()
        self.assertEqual(rec["decision"], "confirmed by robert")

    def test_a_never_cannot_be_confirmed_past(self):
        with self.assertRaises(WarrantError):
            with self.w.act(
                AGENT, "fs.read:/home/robert/.ssh/id_rsa", confirmed_by="robert"
            ):
                pass

    def test_an_exception_in_the_body_is_journalled_as_failed_and_propagates(self):
        # The action was authorised and it ran, so it may have half-happened.
        # That is exactly the case the undo was written down in advance for.
        with self.assertRaises(ZeroDivisionError):
            with self.w.act(
                AGENT,
                "fs.write:/home/robert/a.md",
                undo=("restore a.md", {"backup": "/var/b/2"}),
            ):
                1 / 0

        (rec,) = self.w.history()
        self.assertEqual(rec["outcome"], "failed")
        self.assertIn("ZeroDivisionError", rec["detail"])
        self.assertEqual(rec["undo"]["data"]["backup"], "/var/b/2")


class TestUndo(Base):
    def _write_one(self):
        with self.w.act(
            AGENT,
            "fs.write:/home/robert/a.md",
            undo=("restore a.md", {"backup": "/var/b/3"}),
        ) as a:
            pass
        return a.seq

    def test_the_undo_comes_back_as_the_host_stored_it(self):
        seq = self._write_one()
        note, data = self.w.take_undo(seq)
        self.assertEqual(note, "restore a.md")
        self.assertEqual(data["backup"], "/var/b/3")

    def test_an_undo_is_only_spent_once(self):
        seq = self._write_one()
        self.w.take_undo(seq)
        self.w.reverted(seq, by=0)
        with self.assertRaises(WarrantError):
            self.w.take_undo(seq)
        self.assertIsNone(self.w.undoable())

    def test_a_failed_undo_leaves_the_action_still_undoable(self):
        seq = self._write_one()
        self.w.take_undo(seq)  # host tries, and suppose it fails
        self.assertIsNotNone(self.w.undoable())


class TestIntrospection(Base):
    def test_it_can_list_what_it_will_never_do(self):
        absolutes = self.w.absolutes()
        self.assertEqual(len(absolutes), 1)
        self.assertEqual(absolutes[0]["capability"], "fs.read:/**/.ssh/**")

    def test_it_can_list_what_it_grades(self):
        by_name = {g["capability"]: g["risk"] for g in self.w.grades()}
        self.assertEqual(by_name["fs.delete"], "elevated")

    def test_a_missing_binary_fails_at_construction_not_at_the_first_decision(self):
        with self.assertRaises(WarrantError):
            Warrant(dir=self.dir, binary="warrant-that-does-not-exist")

    def test_the_journal_on_disk_is_one_json_object_per_line(self):
        self.w.rule(AGENT, "fs.write:/home/robert/a.md")
        with self.w.act(AGENT, "fs.write:/home/robert/a.md", undo=("x", {})):
            pass
        with open(os.path.join(self.dir, "journal.ndjson")) as f:
            lines = [ln for ln in f.read().splitlines() if ln.strip()]
        self.assertEqual(len(lines), 2)
        for ln in lines:
            json.loads(ln)


if __name__ == "__main__":
    unittest.main()
