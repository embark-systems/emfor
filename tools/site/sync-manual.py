#!/usr/bin/env python3
"""Turn emfor's docs/manual (or docs/manual.zh_TW) into the Hugo pages of
embark.systems (content.en/emfor or content/emfor), the way the pages were
made by hand: TOML front matter instead of the heading, no previous/next line,
chapter links as Hugo paths, repository links as GitHub URLs at a tag (at main
for a file that the tag does not have), and the
ASCII architecture sketch of the concepts chapter as an image (when the
chapter still has that sketch: the manual of 0.7.0 keeps its own diagrams).
Pages of an earlier sync that the manual no longer has are removed first.

usage: tools/site/sync-manual.py SRC_DIR DST_DIR TAG LANG   (LANG: en or zh)
example, from the root of this repository (the site is a sibling checkout):
  tools/site/sync-manual.py docs/manual ../embark.systems/content.en/emfor v0.7.0 en
  tools/site/sync-manual.py docs/manual.zh_TW ../embark.systems/content/emfor v0.7.0 zh
The images (tools/site/gen-diagrams.py) go to the site's static/images.
"""
import os
import re
import subprocess
import sys

src, dst, tag, lang = sys.argv[1:5]
repo = "https://github.com/embark-systems/emfor"
guide_title = {"en": "0. Guide to the Manual", "zh": "0. 手冊導讀"}[lang]


root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")


def in_tag(path):
    """True when the tag has the file, or when it cannot be asked (no such tag
    in this checkout), so that the link stays pinned to the tag."""
    try:
        subprocess.run(["git", "rev-parse", "-q", "--verify", f"{tag}^{{}}"],
                       cwd=root, check=True, capture_output=True)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return True
    r = subprocess.run(["git", "cat-file", "-e", f"{tag}:{path.rstrip('/')}"],
                       cwd=root, capture_output=True)
    return r.returncode == 0


def github(path):
    """A link to the file at the tag; to main for a file the tag does not have
    yet (a manual of main can name a file that is newer than the release)."""
    kind = "tree" if path.endswith("/") else "blob"
    ref = tag if in_tag(path) else "main"
    return f"{repo}/{kind}/{ref}/{path}"


def rewrite_links(text):
    def repl(m):
        target = m.group(1)
        # a chapter of the manual
        c = re.fullmatch(r"(\d\d-[a-z-]+)\.md(#.*)?", target)
        if c:
            return f"](../{c.group(1)}/{c.group(2) or ''})"
        r = re.fullmatch(r"README\.md(#.*)?", target)
        if r:
            return f"](../00-guide/{r.group(1) or ''})"
        if target.startswith("../../"):
            return f"]({github(target[len('../../'):])})"
        if target.startswith("../") and not target.startswith("../manual"):
            return f"]({github('docs/' + target[len('../'):])})"
        return m.group(0)

    return re.sub(r"\]\(([^)\s]+)\)", repl, text)


ARCH_ALT = {
    "en": "How emfor fits on one host: the daemon checks the backends over the network, writes the local LMDB state store and serves DNS on 127.0.0.1; the client library and emfor status read the store.",
    "zh": "emfor 在一台主機上的組成：daemon 透過網路檢查 backend，寫入本機的 LMDB state store，並在 127.0.0.1 提供 DNS；client 函式庫與 emfor status 讀取 store。",
}
ARCH_SVG = {"en": "/images/emfor-architecture.svg", "zh": "/images/emfor-architecture.zh.svg"}


def replace_architecture(text):
    """the architecture sketch of the concepts chapter becomes an image"""
    fence = "`" * 3
    pat = re.compile(
        re.escape(fence)
        + r"\n[^`]*emfor daemon -+> DNS on 127\.0\.0\.1[^`]*"
        + re.escape(fence)
        + r"\n"
    )
    return pat.sub(lambda m: f"![{ARCH_ALT[lang]}]({ARCH_SVG[lang]})\n", text)


os.makedirs(dst, exist_ok=True)
# the chapters of an earlier sync (the names changed between manuals);
# _index.md of the section is not ours
for old in os.listdir(dst):
    if re.fullmatch(r"\d\d-.*\.md", old):
        os.remove(os.path.join(dst, old))
# The order of the chapters: README first, then by file name without ".md", so
# that 07-dns comes before 07-dns-recipes ('.' sorts after '-'). Every chapter
# gets its own weight, which the site's previous/next links and menu follow.
names = sorted((n for n in os.listdir(src) if n.endswith(".md")), key=lambda n: (n != "README.md", n[:-3]))
for position, name in enumerate(names, start=1):
    lines = open(os.path.join(src, name), encoding="utf-8").read().split("\n")
    assert lines[0].startswith("# "), (name, lines[0])
    if name == "README.md":
        out_name, title, weight = "00-guide.md", guide_title, position
        # the language switch line and the blank line after it, when the
        # README has one (the manual of 0.7.0 has none)
        i = next((i for i, l in enumerate(lines) if "manual.zh_TW/README.md" in l or "manual/README.md" in l), None)
        if i is not None:
            del lines[i : i + 2]
    else:
        out_name = name
        title = lines[0][2:]
        weight = position
        # the previous/next line and the blank line after it
        i = next(i for i, l in enumerate(lines) if "](README.md)" in l)
        assert i < 4, (name, "the previous/next line is not at the top")
        del lines[i : i + 2]
    body = "\n".join(lines[1:])
    body = rewrite_links(body)
    body = replace_architecture(body)
    front = f'+++\ntitle = "{title}"\nweight = {weight}\n+++\n'
    open(os.path.join(dst, out_name), "w", encoding="utf-8").write(front + body)
