import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location("release_notes", Path(__file__).parents[1] / "release_notes.py")
NOTES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTES)


class ReleaseNotesTests(unittest.TestCase):
    def test_folds_prs_and_credits_authors_by_name_once(self):
        github = """## What's Changed
* Add permissions by @Cloud370 in https://github.com/Cloud370/kuku/pull/198
* Fix setup by @Cloud370 in https://github.com/Cloud370/kuku/pull/193
## New Contributors
* @first-user made their first contribution in https://github.com/Cloud370/kuku/pull/193
**Full Changelog**: https://github.com/Cloud370/kuku/compare/old...new
"""
        notes = NOTES.format_details(github, ["Cloud370", "Co-author"])
        self.assertIn("感谢 [@Cloud370](https://github.com/Cloud370)、Co-author", notes)
        self.assertIn("本次首次贡献的开发者有 [@first-user]", notes)
        self.assertIn("<summary>完整 PR 清单（2 项）</summary>", notes)
        self.assertIn("- Add permissions（[#198](https://github.com/Cloud370/kuku/pull/198)）", notes)
        self.assertNotIn(" by @Cloud370", notes)
        self.assertNotIn("Full Changelog", notes)
