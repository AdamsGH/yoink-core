from typing import Any

import pytest
from yoink_insight.commands import tldr as tldr_command
from yoink_insight.config import InsightConfig
from yoink_insight.services import search_client
from yoink_insight.services.search_client import (
    SearchFetchResult,
    SearchSource,
    _extract_search_result,
    _parse_response_payload,
    _responses_endpoint,
    join_sources_for_llm,
)
from yoink_insight.services.tldr import (
    _NOBULLSHIT_PROMPT,
    _build_prompt,
    parse_tldr_request,
    prepare_tldr,
)


class SearchContext:
    def __init__(self) -> None:
        self.bot_data: dict[str, Any] = {"session_factory": object()}


def test_alias_before_focus_request_is_parsed_as_mode() -> None:
    request = parse_tldr_request(
        [
            "https://youtu.be/nF7_bEaHDN4?si=9gl1dZC5fipgdV7x",
            "nobullshit",
            "критически",
            "оцени",
            "доёбы",
            "озвученные",
            "в",
            "видео",
        ],
        {"max", "nobullshit", "noshit", "tale"},
    )

    assert request.url == "https://youtu.be/nF7_bEaHDN4?si=9gl1dZC5fipgdV7x"
    assert request.alias_key == "nobullshit"
    assert request.question == "критически оцени доёбы озвученные в видео"


def test_alias_only_request_remains_cacheable() -> None:
    request = parse_tldr_request(
        ["https://example.com/article", "nobullshit"],
        {"nobullshit"},
    )

    assert request.alias_key == "nobullshit"
    assert request.question is None


def test_free_text_without_alias_remains_a_question() -> None:
    request = parse_tldr_request(
        ["example.com/article", "compare", "the", "benchmarks"],
        {"nobullshit"},
    )

    assert request.url == "https://example.com/article"
    assert request.alias_key is None
    assert request.question == "compare the benchmarks"


def test_nobullshit_prompt_keeps_focus_and_research_separate() -> None:
    prompt = _build_prompt(
        "APFS took 35 seconds while ext4 took 7.5 seconds.",
        "YouTube video",
        "ru",
        "критически оцени методику и выводы",
        alias_instruction=_NOBULLSHIT_PROMPT,
        research="### Source 1: Apple documentation\nURL: https://example.com/apfs\nIndependent data.",
    )

    assert "User's requested focus (mandatory): критически оцени методику и выводы" in prompt
    assert "The source repeating a number is not independent confirmation" in prompt
    assert "Source content:\nAPFS took 35 seconds while ext4 took 7.5 seconds." in prompt
    assert "Independent web research" in prompt
    assert "https://example.com/apfs" in prompt


def test_nobullshit_prompt_forbids_fake_verification_without_research() -> None:
    prompt = _build_prompt(
        "A benchmark claim.",
        "article",
        "en",
        None,
        alias_instruction=_NOBULLSHIT_PROMPT,
    )

    assert "Independent web research: Not supplied. Do not claim external verification." in prompt
    assert "Never invent motives from tone or disagreement" in prompt


@pytest.mark.asyncio
async def test_prepare_tldr_preserves_source_and_adds_independent_research(monkeypatch) -> None:
    captured_query = ""

    async def fake_transcript(url: str, config: InsightConfig) -> tuple[str, int]:
        return "filesystem benchmark transcript", 120

    async def fake_search(query: str, config: InsightConfig, **kwargs) -> SearchFetchResult:
        nonlocal captured_query
        captured_query = query
        return SearchFetchResult(
            sources=[
                SearchSource(
                    url="https://example.com/benchmark-methodology",
                    title="Benchmark methodology",
                    snippet="The comparison used different cache states.",
                )
            ],
            via="openai-responses:web_search",
            answer="The comparison used different cache states.",
        )

    monkeypatch.setattr("yoink_insight.services.tldr._fetch_youtube_transcript", fake_transcript)
    monkeypatch.setattr("yoink_insight.services.tldr.search_fetch", fake_search)

    prepared = await prepare_tldr(
        "https://youtu.be/example",
        InsightConfig(tldr_max_content_chars=10_000),
        use_search=True,
        question="check the benchmark controls",
    )

    assert prepared.content == "filesystem benchmark transcript"
    assert "different cache states" in (prepared.research or "")
    assert prepared.research_via == "openai-responses:web_search"
    assert "check the benchmark controls" in captured_query
    assert "Do not use the source itself as corroboration" in captured_query


@pytest.mark.asyncio
async def test_search_requires_setting_and_effective_grant(monkeypatch) -> None:
    class Settings:
        async def get_use_search(self, user_id: int) -> bool:
            return True

    class Resolver:
        def __init__(self, session_factory, bot_data) -> None:
            pass

        async def is_allowed(self, user_id: int, plugin: str, feature: str) -> bool:
            return (plugin, feature) == ("insight", "search")

    monkeypatch.setattr(tldr_command, "EffectiveFeatureResolver", Resolver)
    context = SearchContext()

    assert await tldr_command._should_use_search(context, Settings(), 42, None) is True


@pytest.mark.asyncio
async def test_search_is_disabled_for_byok_route() -> None:
    class Settings:
        async def get_use_search(self, user_id: int) -> bool:
            raise AssertionError("BYOK must bypass gateway search settings")

    context = SearchContext()
    byok = tldr_command.ByokRoute("openai", None, "key", "model")

    assert await tldr_command._should_use_search(context, Settings(), 42, byok) is False


def test_native_search_response_extracts_answer_and_citations() -> None:
    payload = _parse_response_payload(
        '{"output":[{"type":"message","content":[{"type":"output_text",'
        '"text":"APFS metadata behavior is documented.","annotations":[{'
        '"type":"url_citation","url":"https://example.com/apfs","title":"APFS docs",'
        '"start_index":0,"end_index":13}]}]},{"type":"web_search_call",'
        '"action":{"sources":[{"url":"https://example.com/apfs","title":"APFS docs"}]}}]}'
    )

    answer, sources = _extract_search_result(payload)
    joined = join_sources_for_llm(
        SearchFetchResult(sources=sources, via="openai-responses:web_search", answer=answer)
    )

    assert answer == "APFS metadata behavior is documented."
    assert [source.url for source in sources] == ["https://example.com/apfs"]
    assert "Grounded web-search synthesis" in joined
    assert "URL: https://example.com/apfs" in joined


def test_responses_endpoint_preserves_v1_base_path() -> None:
    assert _responses_endpoint("http://a2a:8317/v1/") == "http://a2a:8317/v1/responses"


@pytest.mark.asyncio
async def test_search_fetch_uses_native_web_search_tool(monkeypatch) -> None:
    captured: dict = {}

    class Response:
        status_code = 200
        text = (
            '{"output":[{"type":"message","content":[{"type":"output_text",'
            '"text":"Grounded answer","annotations":[]}]}]}'
        )

    class Client:
        def __init__(self, timeout: float) -> None:
            captured["timeout"] = timeout

        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, traceback) -> None:
            return None

        async def post(self, endpoint: str, *, json: dict, headers: dict):
            captured.update(endpoint=endpoint, body=json, headers=headers)
            return Response()

    monkeypatch.setattr(search_client.httpx, "AsyncClient", Client)
    config = InsightConfig(
        gateway_base_url="http://a2a:8317/v1",
        gateway_api_key="test-key",
        tldr_search_timeout_seconds=37,
    )

    result = await search_client.search_fetch("fact check", config, model="gpt-test")

    assert captured["endpoint"] == "http://a2a:8317/v1/responses"
    assert captured["timeout"] == 37
    assert captured["body"]["tools"] == [{"type": "web_search"}]
    assert captured["body"]["tool_choice"] == "required"
    assert result.answer == "Grounded answer"
