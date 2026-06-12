"""Unit tests for yoink_inbox.services.ingest.

Cover the pure functions only (URL normalisation and kind detection); the
DB / ARQ path is exercised separately once a per-plugin fixture lands.
"""
from __future__ import annotations

import pytest
from yoink_inbox.services.ingest import detect_kind, normalize_url


class TestNormalizeUrl:
    def test_lowercases_scheme_and_host(self) -> None:
        assert normalize_url("HTTPS://Example.COM/Path") == "https://example.com/Path"

    def test_strips_fragment(self) -> None:
        assert normalize_url("https://example.com/#section") == "https://example.com/"

    def test_strips_utm_params(self) -> None:
        out = normalize_url(
            "https://example.com/post?utm_source=tg&utm_medium=bot&keep=1"
        )
        assert out == "https://example.com/post?keep=1"

    def test_strips_gclid_and_fbclid(self) -> None:
        out = normalize_url("https://example.com/?gclid=abc&fbclid=def&page=2")
        assert out == "https://example.com/?page=2"

    def test_strips_youtube_si_tracking(self) -> None:
        out = normalize_url("https://youtu.be/dQw4w9WgXcQ?si=trackingblob")
        assert out == "https://youtu.be/dQw4w9WgXcQ"

    def test_sorts_query_params(self) -> None:
        out = normalize_url("https://example.com/?b=2&a=1&c=3")
        assert out == "https://example.com/?a=1&b=2&c=3"

    def test_drops_default_ports(self) -> None:
        assert normalize_url("https://example.com:443/x") == "https://example.com/x"
        assert normalize_url("http://example.com:80/x") == "http://example.com/x"

    def test_keeps_non_default_port(self) -> None:
        assert normalize_url("http://localhost:8080/api") == "http://localhost:8080/api"

    def test_strips_trailing_slash_from_path(self) -> None:
        # Karakeep treats /post and /post/ as the same item.
        assert normalize_url("https://example.com/post/") == "https://example.com/post"
        # But the root must keep its slash, otherwise we lose the host marker.
        assert normalize_url("https://example.com/") == "https://example.com/"

    def test_bare_host_defaults_to_https(self) -> None:
        assert normalize_url("github.com/foo/bar") == "https://github.com/foo/bar"

    def test_identical_inputs_dedup(self) -> None:
        a = normalize_url("https://github.com/foo/bar?utm_source=x#readme")
        b = normalize_url("HTTPS://GitHub.com/foo/bar/")
        assert a == b


class TestDetectKind:
    def test_github_repo(self) -> None:
        assert detect_kind("https://github.com/lazyvim/lazyvim") == "github_repo"
        assert detect_kind("https://github.com/lazyvim/lazyvim/issues") == "github_repo"

    def test_github_user_root_is_not_a_repo(self) -> None:
        # /lazyvim alone is the user/org page; we treat it as a plain link.
        assert detect_kind("https://github.com/lazyvim") == "link"

    def test_youtube(self) -> None:
        assert detect_kind("https://youtu.be/dQw4w9WgXcQ") == "video"
        assert detect_kind("https://www.youtube.com/watch?v=abc") == "video"

    def test_vimeo(self) -> None:
        assert detect_kind("https://vimeo.com/12345") == "video"

    def test_generic_link(self) -> None:
        assert detect_kind("https://example.com/article") == "link"


@pytest.mark.parametrize(
    "url, expected",
    [
        ("https://example.com/?_hsenc=abc&q=1", "https://example.com/?q=1"),
        ("https://example.com/?mc_cid=x&keep=2", "https://example.com/?keep=2"),
        ("https://example.com/?ref_src=twitter&id=7", "https://example.com/?id=7"),
    ],
)
def test_additional_tracking_keys_stripped(url: str, expected: str) -> None:
    assert normalize_url(url) == expected
