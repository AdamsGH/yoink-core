"""Real yt-dlp PoToken provider using bgutil v2 through the confined adapter."""
import json
import os
from youtube_auth import call, SOCKET_ENV


def install():
    if not os.environ.get(SOCKET_ENV):
        return
    from yt_dlp.extractor.youtube.pot.provider import (
        PoTokenProvider, PoTokenContext, PoTokenResponse, PoTokenProviderError,
        register_provider, register_preference,
    )
    from yt_dlp.extractor.youtube.pot.utils import WEBPO_CLIENTS, get_webpo_content_binding

    @register_provider
    class YoinkBgutilPTP(PoTokenProvider):
        PROVIDER_NAME = "yoink:bgutil"
        _SUPPORTED_CLIENTS = WEBPO_CLIENTS
        _SUPPORTED_CONTEXTS = (PoTokenContext.GVS, PoTokenContext.PLAYER, PoTokenContext.SUBS)
        # Do not allow bgutil to bypass configured proxy/TLS/source-address policies.
        _SUPPORTED_EXTERNAL_REQUEST_FEATURES = ()

        def is_available(self):
            return bool(os.environ.get(SOCKET_ENV))

        def _real_request_pot(self, request):
            if request.internal_client_name == "web_music":
                raise PoTokenProviderError("bgutil music challenge unavailable")
            try:
                version = json.loads(call("/pot/ping", {}, 5, 4096))
                if not str(version.get("version", "")).startswith("2."):
                    raise ValueError()
                value = json.loads(call("/pot", {
                    "bypass_cache": request.bypass_cache, "challenge": None,
                    "content_binding": get_webpo_content_binding(request)[0],
                    "disable_tls_verification": False, "proxy": None,
                    "innertube_context": request.innertube_context, "source_address": None,
                }, 20, 64 * 1024))
                token = value.get("poToken")
                if not isinstance(token, str) or not token or len(token) > 16384 or any(ord(c) < 32 for c in token):
                    raise ValueError()
                return PoTokenResponse(po_token=token)
            except Exception:
                raise PoTokenProviderError("confined bgutil request failed") from None

    @register_preference(YoinkBgutilPTP)
    def preference(provider, request):
        return 130
