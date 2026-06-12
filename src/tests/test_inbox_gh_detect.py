"""Unit tests for yoink_inbox.services.gh_detect."""
from __future__ import annotations

import pytest
from yoink_inbox.services.gh_detect import (
    canonical_repo_url,
    is_github_url,
    parse_repo,
)


class TestIsGithubUrl:
    @pytest.mark.parametrize(
        "url",
        [
            "https://github.com/lazyvim/lazyvim",
            "https://www.github.com/foo",
            "HTTP://GITHUB.COM/x/y",
        ],
    )
    def test_positive(self, url: str) -> None:
        assert is_github_url(url) is True

    @pytest.mark.parametrize(
        "url",
        [
            "https://gitlab.com/foo/bar",
            "https://raw.githubusercontent.com/foo/bar/main/README.md",
            "https://example.com/",
            "not even a url",
        ],
    )
    def test_negative(self, url: str) -> None:
        assert is_github_url(url) is False


class TestParseRepo:
    def test_clean_repo_root(self) -> None:
        ref = parse_repo("https://github.com/lazyvim/lazyvim")
        assert ref is not None
        assert ref.owner == "lazyvim"
        assert ref.repo == "lazyvim"
        assert ref.full_name == "lazyvim/lazyvim"

    def test_with_subpath(self) -> None:
        ref = parse_repo("https://github.com/lazyvim/lazyvim/tree/main/lua")
        assert ref is not None
        assert ref.full_name == "lazyvim/lazyvim"

    def test_user_page_is_not_a_repo(self) -> None:
        assert parse_repo("https://github.com/lazyvim") is None

    def test_strips_dot_git_clone_url(self) -> None:
        ref = parse_repo("https://github.com/lazyvim/lazyvim.git")
        assert ref is not None
        assert ref.repo == "lazyvim"

    def test_meta_paths_are_not_repos(self) -> None:
        for u in (
            "https://github.com/orgs/karakeep-app",
            "https://github.com/settings/profile",
            "https://github.com/marketplace/actions/foo",
            "https://github.com/topics/typescript",
            "https://github.com/users/me",
            "https://github.com/search?q=foo",
        ):
            assert parse_repo(u) is None, u

    def test_non_github_returns_none(self) -> None:
        assert parse_repo("https://gitlab.com/foo/bar") is None


class TestCanonicalRepoUrl:
    def test_subpath_collapses(self) -> None:
        out = canonical_repo_url("https://github.com/foo/bar/issues/42")
        assert out == "https://github.com/foo/bar"

    def test_non_repo_is_none(self) -> None:
        assert canonical_repo_url("https://github.com/foo") is None
