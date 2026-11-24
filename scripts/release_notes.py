"""Format the attribution and PR details appended to the Chinese announcement."""

from html import escape
from pathlib import Path
import re
import sys


PR = re.compile(r"^[-*] (.+) by @([\w-]+) in (https://\S+/pull/([0-9]+))$")
FIRST_CONTRIBUTION = re.compile(r"^[-*] @([\w-]+) made their first contribution in https://\S+/pull/[0-9]+$")


def format_details(generated_notes, commit_authors):
    contributors = {name.casefold(): escape(name) for name in commit_authors if name}
    changes = []
    first_contributors = []
    pr_count = 0
    for line in generated_notes.splitlines():
        if line in ("## What's Changed", "## New Contributors") or line.startswith("**Full Changelog**:"):
            continue
        if pr := PR.fullmatch(line):
            title, author, url, number = pr.groups()
            contributors[author.casefold()] = f"[@{author}](https://github.com/{author})"
            changes.append(f"- {escape(title)}（[#{number}]({url})）")
            pr_count += 1
        elif first := FIRST_CONTRIBUTION.fullmatch(line):
            author = first.group(1)
            link = f"[@{author}](https://github.com/{author})"
            contributors[author.casefold()] = link
            first_contributors.append(link)
        else:
            changes.append(line)

    sections = []
    if contributors:
        names = "、".join(contributors[key] for key in sorted(contributors))
        sections.append(f"## 贡献者\n\n感谢 {names} 的贡献。")
    if first_contributors:
        sections.append("本次首次贡献的开发者有 " + "、".join(first_contributors) + "。")
    details = "\n".join(changes).strip()
    if details:
        label = f"完整 PR 清单（{pr_count} 项）" if pr_count else "完整 PR 清单"
        sections.append(f"<details>\n<summary>{label}</summary>\n\n{details}\n\n</details>")
    return "\n\n".join(sections)


if __name__ == "__main__":
    notes, authors = map(Path, sys.argv[1:])
    print(format_details(notes.read_text(encoding="utf-8"), authors.read_text(encoding="utf-8").splitlines()))
