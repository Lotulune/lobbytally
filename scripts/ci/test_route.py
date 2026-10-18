import copy
from pathlib import Path
import subprocess
import tempfile
import unittest

from route import FULL_JOBS, NORMAL_JOBS, failed_results, is_document, route


class PathClassificationTests(unittest.TestCase):
    def test_only_known_documentation_is_allowed(self):
        for path in ["README.md", "README.en.md", "AGENTS.md", "web/README.md",
                     "docs/AGENT_DEPLOYMENT.md", "docs/guide with spaces.md",
                     "docs/嵌套/说明.md", "docs/images/screen shot.png"]:
            with self.subTest(path=path):
                self.assertTrue(is_document(path))
        for path in ["CONTEXT.md", "README.py", "other/README.md", "src/AGENTS.md",
                     "docs/config.yml", "docs/script.py", "docs/images/code.js",
                     "docs/images/icon.png.ts", "web/public/image.png",
                     ".github/workflows/README.md", "supabase/migrations/README.md",
                     "docs/../Cargo.toml", "/docs/guide.md", "docs//guide.md",
                     "docs/line\nbreak.md", "docs/tab\tname.md", "docs\\guide.md"]:
            with self.subTest(path=path):
                self.assertFalse(is_document(path))


class GitRoutingTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.git("init", "--quiet")
        self.git("config", "user.name", "CI route test")
        self.git("config", "user.email", "ci-route@example.invalid")
        self.write("README.md", "initial\n")
        self.write("src/main.rs", "fn main() {}\n")
        self.base = self.commit()

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True,
                              capture_output=True).stdout.decode("utf-8").strip()

    def write(self, name, content):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def commit(self):
        self.git("add", "--all")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "--quiet", "--no-gpg-sign", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def push_route(self, head, **extra):
        return route(self.repo, "push", {"before": self.base, "after": head, **extra}, "refs/heads/main")

    def test_document_only_diff_handles_spaces_unicode_and_deletion(self):
        (self.repo / "README.md").unlink()
        self.write("docs/说明 with spaces.md", "guide\n")
        result = self.push_route(self.commit())
        self.assertEqual(result["run_checks"], "false")

    def test_mixed_change_and_code_deletion_run_normal_checks(self):
        self.write("README.md", "updated\n")
        (self.repo / "src/main.rs").unlink()
        self.assertEqual(self.push_route(self.commit())["run_checks"], "true")

    def test_rename_code_to_document_keeps_old_code_path_in_scope(self):
        (self.repo / "docs").mkdir()
        self.git("mv", "src/main.rs", "docs/example.md")
        self.assertEqual(self.push_route(self.commit())["run_checks"], "true")

    def test_new_branch_unknown_base_empty_diff_and_bad_revision_fail_closed(self):
        self.write("README.md", "updated\n")
        head = self.commit()
        cases = [
            {"before": "0" * 40, "after": head},
            {"before": "f" * 40, "after": head},
            {"before": head, "after": head},
            {"before": self.base, "after": head, "created": True},
            {"before": self.base, "after": "0" * 40, "deleted": True},
            {"before": "--help", "after": head},
        ]
        for event in cases:
            with self.subTest(event=event):
                self.assertEqual(route(self.repo, "push", event, "refs/heads/main")["run_checks"], "true")

    def test_pull_request_compares_merge_base_instead_of_unrelated_base_work(self):
        self.git("checkout", "--quiet", "-b", "documentation")
        self.write("README.md", "PR documentation\n")
        head = self.commit()
        self.git("checkout", "--quiet", "--detach", self.base)
        self.write("src/main.rs", "fn main() { println!(\"base branch\"); }\n")
        target = self.commit()
        event = {"pull_request": {"base": {"sha": target}, "head": {"sha": head}}}
        result = route(self.repo, "pull_request", event, "refs/pull/1/merge")
        self.assertEqual(result["run_checks"], "false")

    def test_large_diff_is_not_truncated_to_first_documentation_paths(self):
        for index in range(305):
            self.write(f"docs/{index:03d}.md", "guide\n")
        self.write("src/main.rs", "fn main() { }\n")
        self.assertEqual(self.push_route(self.commit())["run_checks"], "true")

    def test_manual_and_tag_runs_do_not_use_document_bypass(self):
        for event_name, event, ref, full in [
            ("workflow_dispatch", {"inputs": {"full": False}}, "refs/heads/main", "false"),
            ("workflow_dispatch", {"inputs": {"full": "true"}}, "refs/heads/main", "true"),
            ("push", {}, "refs/tags/v1.0.0", "true"),
            ("merge_group", {}, "refs/heads/main", "false"),
        ]:
            with self.subTest(event_name=event_name, ref=ref):
                result = route(self.repo, event_name, event, ref)
                self.assertEqual(result["run_checks"], "true")
                self.assertEqual(result["run_full"], full)


class ResultGateTests(unittest.TestCase):
    def needs(self, normal, full):
        needs = {"route": {"result": "success", "outputs": {
            "run_checks": str(normal).lower(), "run_full": str(full).lower(),
            "reason": "code_or_unknown_paths" if normal else "documentation_only"}}}
        needs.update({job: {"result": "success" if normal else "skipped"} for job in NORMAL_JOBS})
        needs.update({job: {"result": "success" if full else "skipped"} for job in FULL_JOBS})
        return needs

    def test_documentation_normal_and_full_routes_have_valid_terminal_results(self):
        for normal, full in [(False, False), (True, False), (True, True)]:
            self.assertEqual(failed_results(self.needs(normal, full)), [])

    def test_failure_cancellation_missing_job_and_unexpected_skip_fail(self):
        for result in ["failure", "cancelled", "skipped", "missing"]:
            needs = self.needs(True, False)
            if result == "missing":
                del needs["quality"]
            else:
                needs["quality"]["result"] = result
            self.assertTrue(failed_results(needs), result)

    def test_route_failure_or_invalid_outputs_cannot_turn_gate_green(self):
        needs = self.needs(False, False)
        for broken in [{}, {"result": "failure"}, {"result": "success", "outputs": {}},
                       {"result": "success", "outputs": {"run_checks": "false", "run_full": "false"}},
                       {"result": "success", "outputs": {"run_checks": "false", "run_full": "true"}}]:
            changed = copy.deepcopy(needs)
            changed["route"] = broken
            self.assertTrue(failed_results(changed))


if __name__ == "__main__":
    unittest.main()
