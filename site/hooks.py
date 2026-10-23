"""MkDocs hooks for the Ligature Protocol site (LP-137).

The documentation under docs/ links to files elsewhere in the repository (the JSON
Schemas, the licence files, the implementation's READMEs) and to parts of docs/ the
site leaves out (the decisions, the milestone plans, the security audits), since the
site is for users and for presenting the protocol. A relative link to any of those is
rewritten to the file on GitHub before the page is rendered. Everything else is left
to MkDocs, which checks it.
"""

import posixpath
import re

REPO = "https://github.com/garth/ligature-protocol"
BRANCH = "main"
# Directories under docs/ that the site does not publish (mkdocs.yml exclude_docs).
UNPUBLISHED = ("adr/", "milestones/", "security-audit/")
LINK = re.compile(r"(\]\()([^)\s#]+)(#[^)]*)?(\))")


def on_page_markdown(markdown, page, config, files):
    """Rewrites links that leave docs/ to their GitHub pages."""
    page_dir = posixpath.dirname(page.file.src_uri)

    def rewrite(match):
        open_, target, fragment, close = match.groups()
        fragment = fragment or ""
        if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("/"):
            return match.group(0)
        resolved = posixpath.normpath(posixpath.join(page_dir, target))
        outside = resolved.startswith("../")
        unpublished = any(resolved.startswith(d) for d in UNPUBLISHED)
        if not outside and not unpublished:
            return match.group(0)
        repo_path = posixpath.normpath(posixpath.join("docs", page_dir, target))
        kind = "tree" if target.endswith("/") else "blob"
        return f"{open_}{REPO}/{kind}/{BRANCH}/{repo_path}{fragment}{close}"

    return LINK.sub(rewrite, markdown)
