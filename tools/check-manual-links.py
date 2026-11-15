#!/usr/bin/env python3
"""Check the links of the manual: every link to another chapter (or to a
heading of the same chapter) must name a file that exists and a heading whose
GitHub anchor matches. Links to repository files outside docs/manual are only
checked for existence.

usage: tools/check-manual-links.py [DIR ...]   (default: docs/manual docs/manual.zh_TW)
exit status 0 when every link is fine, 1 otherwise.
"""
import os
import re
import sys
import unicodedata
import urllib.parse


def slug(heading):
    """GitHub's anchor of a heading: lower case, punctuation removed, spaces
    become hyphens."""
    heading = re.sub(r"`", "", heading).strip().lower()
    out = []
    for ch in heading:
        if ch == " ":
            out.append("-")
        elif ch in "-_" or unicodedata.category(ch)[0] in "LN":
            out.append(ch)
    return "".join(out)


def headings(path):
    anchors = {}
    in_code = False
    for line in open(path, encoding="utf-8"):
        if line.startswith("```"):
            in_code = not in_code
        m = None if in_code else re.match(r"#{1,6}\s+(.*?)\s*#*\s*$", line)
        if m:
            base = slug(m.group(1))
            n = anchors.get(base, 0)
            anchors[base] = n + 1
            if n:
                anchors[f"{base}-{n}"] = 1
    return set(anchors)


dirs = sys.argv[1:] or ["docs/manual", "docs/manual.zh_TW"]
problems = 0
checked = 0
for d in dirs:
    for name in sorted(os.listdir(d)):
        if not name.endswith(".md"):
            continue
        path = os.path.join(d, name)
        text = open(path, encoding="utf-8").read()
        text = re.sub(r"```.*?```", "", text, flags=re.S)
        for m in re.finditer(r"\]\(([^)\s]+)\)", text):
            target = m.group(1)
            if re.match(r"[a-z]+://|mailto:", target):
                continue
            file_part, _, anchor = target.partition("#")
            dest = os.path.normpath(os.path.join(d, file_part)) if file_part else path
            checked += 1
            if not os.path.exists(dest):
                print(f"{path}: {target}: no such file")
                problems += 1
                continue
            if anchor and dest.endswith(".md"):
                if urllib.parse.unquote(anchor).lower() not in headings(dest):
                    print(f"{path}: {target}: no heading with that anchor in {dest}")
                    problems += 1
print(f"{checked} links, {problems} problems")
sys.exit(1 if problems else 0)
