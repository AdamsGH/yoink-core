"""GitHub URL detection and canonicalisation.

Shared between ingest (cheap kind classification) and the classify / star
pipeline (where we need owner+repo to hit /repos/{owner}/{repo} and to bridge
inbox items to GH-star rows).
"""
from __future__ import annotations

from dataclasses import dataclass
from urllib.parse import urlsplit

_GH_HOSTS = frozenset({"github.com", "www.github.com"})

# Path segments that follow /owner/repo and should be stripped when we
# canonicalise to a repo root (e.g. /tree/main/README.md, /issues/42).
_SUB_PATHS = frozenset({
    "tree", "blob", "issues", "pulls", "actions", "wiki", "releases",
    "discussions", "projects", "security", "pulse", "graphs", "commits",
    "commit", "compare", "settings", "labels", "milestones", "tags",
    "branches", "network", "stargazers", "watchers", "forks",
})


@dataclass(slots=True, frozen=True)
class GhRepoRef:
    owner: str
    repo: str

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.repo}"

    @property
    def canonical_url(self) -> str:
        return f"https://github.com/{self.owner}/{self.repo}"


def is_github_url(url: str) -> bool:
    """True for any github.com URL (including users, repos, issues, raw)."""
    try:
        host = (urlsplit(url).hostname or "").lower()
    except ValueError:
        return False
    return host in _GH_HOSTS


def parse_repo(url: str) -> GhRepoRef | None:
    """Return (owner, repo) for a GitHub URL that resolves to a repository.

    Accepts any sub-path under /owner/repo/... and reduces it to the repo
    root. Returns None for /owner alone (a user/org page), for the website
    root, or for non-github URLs.
    """
    try:
        s = urlsplit(url)
    except ValueError:
        return None
    if (s.hostname or "").lower() not in _GH_HOSTS:
        return None
    parts = [p for p in s.path.split("/") if p]
    if len(parts) < 2:
        return None
    owner, repo = parts[0], parts[1]
    # Some GitHub paths share the leading /<owner>/<thing> shape but `<thing>`
    # is not actually a repo (e.g. /orgs/foo, /settings/profile, /apps/...).
    if owner in {"orgs", "apps", "marketplace", "settings", "topics", "users", "search"}:
        return None
    # Strip trailing .git on clone URLs.
    if repo.endswith(".git"):
        repo = repo[: -len(".git")]
    return GhRepoRef(owner=owner, repo=repo)


def canonical_repo_url(url: str) -> str | None:
    """Lower a `github.com/owner/repo/...` URL to its canonical repo root."""
    ref = parse_repo(url)
    return ref.canonical_url if ref is not None else None
