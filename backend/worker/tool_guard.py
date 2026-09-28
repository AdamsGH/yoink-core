"""Safe argument construction for image-owned ffmpeg and ffprobe wrappers."""
from __future__ import annotations

import os


_PROTOCOLS = "file,pipe"
_FORMATS = "3gp,aac,ac3,aiff,alac,amr,ape,asf,ass,avi,avr,bmp,caf,cdg,daud,dv,ffm,flac,flv,gif,gsm,h261,h263,h264,hevc,ico,iff,ilbc,image2,image2pipe,jpeg,jpg,ljpeg,m4a,m4v,matroska,mjpeg,mlp,mov,mp3,mp4,mpeg,mpegts,mpg,oga,ogg,ogv,opus,png,rm,roq,s16be,s16le,s24be,s24le,s32be,s32le,s8,smjpeg,sox,spx,srt,svg,swf,truehd,tta,txd,vob,w64,wav,webm,webp,wma,wmv,wtv,yuv4mpegpipe,png_pipe,jpeg_pipe,webp_pipe"
_SAFE_OPTIONS = ["-protocol_whitelist", _PROTOCOLS, "-format_whitelist", _FORMATS]
_BINARIES = {"ffmpeg": "/usr/bin/ffmpeg", "ffprobe": "/usr/bin/ffprobe"}


class WhitelistOverrideError(ValueError):
    """Raised when an invocation attempts to replace a protected whitelist."""


def guarded_arguments(tool: str, args: list[str]) -> list[str]:
    """Add fixed input allowlists and reject any caller-provided overrides."""
    if tool not in _BINARIES:
        raise ValueError("unsupported media tool")
    if any(
        value in ("-protocol_whitelist", "-format_whitelist")
        or value.startswith(("-protocol_whitelist=", "-format_whitelist="))
        for value in args
    ):
        raise WhitelistOverrideError("caller whitelist overrides are denied")

    # Tool/version discovery has no input and must remain compatible with yt-dlp.
    if "-version" in args and "-i" not in args:
        return list(args)
    if tool == "ffmpeg":
        guarded: list[str] = []
        for value in args:
            if value == "-i":
                guarded.extend(_SAFE_OPTIONS)
            guarded.append(value)
        return guarded

    # ffprobe accepts the input as a positional argument. Put input options first,
    # before its other arguments and the final input path.
    if not args:
        return list(args)
    return [*_SAFE_OPTIONS, *args]


def run(tool: str, args: list[str]) -> None:
    """Replace this wrapper process with the fixed Debian media binary."""
    binary = _BINARIES.get(tool)
    if binary is None:
        raise ValueError("unsupported media tool")
    try:
        guarded = guarded_arguments(tool, args)
    except WhitelistOverrideError:
        raise SystemExit(125) from None
    os.execv(binary, [binary, *guarded])
