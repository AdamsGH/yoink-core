"""Focused source and local-ffmpeg tests for media_engine."""
from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

import media_engine
import tool_guard


class MediaEngineTests(unittest.TestCase):
    def test_format_selection_honors_height_and_audio_codec(self):
        self.assertIn("[height<=720]", media_engine._format_expression(
            "video", {"max_height": 720, "video_codec": "h264", "container": "mp4", "audio_codec": "best"}
        ))
        self.assertIn("[vcodec^=av01]", media_engine._format_expression(
            "best", {"max_height": None, "video_codec": "av1", "container": "mkv", "audio_codec": "aac"}
        ))
        self.assertEqual(
            media_engine._format_expression("audio", {"audio_codec": "opus"}),
            "bestaudio[acodec*=opus]/bestaudio/best",
        )

    def test_playlist_indices_preserve_order_and_sign_rules(self):
        self.assertEqual(media_engine.playlist_indices({"start": 4, "end": 2}), [4, 3, 2])
        self.assertEqual(media_engine.playlist_indices({"start": -3, "end": -1}), [-3, -2, -1])
        self.assertEqual(media_engine.playlist_indices({"start": -3, "end": None}), [-3, -2, -1])
        self.assertEqual(media_engine.playlist_indices({"start": 7, "end": None}), [7])
        with self.assertRaises(media_engine.MediaFailure):
            media_engine.playlist_indices({"start": 1, "end": -1})

    def test_media_type_handles_comma_separated_formats_and_webm_opus(self):
        audio = {"streams": [{"codec_type": "audio", "codec_name": "aac"}]}
        self.assertEqual(media_engine._media_type(audio, "mov,mp4,m4a,3gp,3g2,mj2"), "audio/mp4")
        opus = {"streams": [{"codec_type": "audio", "codec_name": "opus"}]}
        self.assertEqual(media_engine._media_type(opus, "matroska,webm"), "audio/webm")
        video = {"streams": [{"codec_type": "video", "codec_name": "h264"}]}
        self.assertEqual(media_engine._media_type(video, "matroska,webm"), "video/x-matroska")

    def test_compatible_video_is_identified_from_probe_metadata(self):
        info = {"streams": [
            {"codec_type": "video", "codec_name": "h264", "height": 720},
            {"codec_type": "audio", "codec_name": "aac"},
        ]}
        options = {"container": "mp4", "video_codec": "h264", "audio_codec": "aac", "max_height": 720}
        self.assertTrue(media_engine._already_compatible(info, "mov,mp4,m4a", options))
        self.assertFalse(media_engine._already_compatible(info, "mov,mp4,m4a", {**options, "max_height": 480}))

    def test_audio_outputs_use_extensions_and_containers_matching_codecs(self):
        for codec, extension, expected_format in (("mp3", "mp3", "mp3"), ("opus", "opus", "opus"), ("aac", "m4a", "mp4"), ("best", "m4a", "mp4")):
            args = media_engine._ffmpeg_args("ffmpeg", Path("source.mkv"), Path(f"audio.{extension}"), {"audio_codec": codec}, "audio")
            self.assertEqual(args[-1], f"audio.{extension}")
            self.assertEqual(args[args.index("-f") + 1], expected_format)
            self.assertIn("-threads", args)

    def test_height_is_enforced_by_scale_filter(self):
        args = media_engine._ffmpeg_args("ffmpeg", Path("source.mp4"), Path("out.mp4"), {"max_height": 480, "video_codec": "h264", "audio_codec": "best"}, "video")
        self.assertIn("scale=-2:min(ih\\,480)", args)

    def test_aggregate_gallery_transfer_limit_rejects_before_overflow_write(self):
        class Sink:
            def __init__(self):
                self.data = bytearray()
            def write(self, data):
                self.data.extend(data)

        sink = Sink()
        used = media_engine._bounded_transfer_write(sink, b"1234", 0, 0, 8, 6)
        self.assertEqual(used, 4)
        with self.assertRaises(media_engine.MediaFailure) as caught:
            media_engine._bounded_transfer_write(sink, b"xyz", 0, used, 8, 6)
        self.assertEqual(caught.exception.failure_class, "resource_limit")
        self.assertEqual(sink.data, b"1234")

    def test_processing_command_only_uses_local_source_and_safe_wrapper(self):
        options = {"container": "mp4", "video_codec": "h264", "audio_codec": "best"}
        command = media_engine._ffmpeg_args("/trusted/ffmpeg", Path("local.mp4"), Path("clip.mp4"), options, "video",
                                            {"start_seconds": 1.0, "end_seconds": 2.5})
        self.assertEqual(command[0], "/trusted/ffmpeg")
        self.assertIn("local.mp4", command)
        self.assertNotIn("https://", " ".join(command))
        self.assertIn("-t", command)
        self.assertEqual(media_engine._PROTOCOLS, "file,pipe")
        self.assertNotIn("hls", media_engine._FORMATS)
        self.assertNotIn("concat", media_engine._FORMATS)

    def test_static_tools_and_guarded_invocation_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            ffmpeg, ffprobe, wrapper_dir = media_engine._tools(workspace)
            self.assertEqual(Path(ffmpeg), wrapper_dir / "ffmpeg")
            self.assertEqual(Path(ffprobe), wrapper_dir / "ffprobe")
            self.assertNotEqual(wrapper_dir, workspace)
            self.assertEqual(list(workspace.iterdir()), [])
            guarded = tool_guard.guarded_arguments("ffmpeg", ["-i", "one.mp4", "-i", "two.mp4"])
            safe = ["-protocol_whitelist", "file,pipe", "-format_whitelist", media_engine._FORMATS]
            self.assertEqual(guarded, [*safe, "-i", "one.mp4", *safe, "-i", "two.mp4"])
            with self.assertRaises(tool_guard.WhitelistOverrideError):
                tool_guard.guarded_arguments("ffmpeg", ["-protocol_whitelist=file,pipe", "-i", "input.mp4"])
            with self.assertRaises(tool_guard.WhitelistOverrideError):
                tool_guard.guarded_arguments("ffprobe", ["-format_whitelist", "all", "input.mp4"])
            self.assertEqual(tool_guard.guarded_arguments("ffmpeg", ["-version"]), ["-version"])
            self.assertFalse(any(path.stat().st_mode & 0o111 for path in workspace.iterdir()))

    def test_real_ffmpeg_generated_video_clip_and_audio(self):
        if not all(__import__("shutil").which(tool) for tool in ("ffmpeg", "ffprobe")):
            self.skipTest("container ffmpeg tools unavailable")
        with tempfile.TemporaryDirectory() as directory:
            old = Path.cwd()
            try:
                os.chdir(directory)
                workspace = Path.cwd()
                ffmpeg, ffprobe, _wrapper = media_engine._tools(workspace)
                media_engine._bounded_output([
                    __import__("shutil").which("ffmpeg"), "-nostdin", "-y", "-f", "lavfi", "-i",
                    "color=c=blue:s=64x64:d=1", "-f", "lavfi", "-i",
                    "sine=frequency=440:duration=1", "-shortest", "-c:v", "libx264",
                    "-pix_fmt", "yuv420p", "-c:a", "aac", "generated.mp4",
                ], 20)
                options = {"container": "mp4", "video_codec": "h264", "audio_codec": "aac"}
                clip = Path("clip.mp4")
                media_engine._bounded_output(media_engine._ffmpeg_args(
                    ffmpeg, Path("generated.mp4"), clip, options, "video",
                    {"start_seconds": 0.1, "end_seconds": 0.7},
                ), 20)
                media_engine._probe(clip, ffprobe, 10)
                for codec, extension, mime in (
                    ("best", "m4a", "audio/mp4"), ("aac", "m4a", "audio/mp4"),
                    ("mp3", "mp3", "audio/mpeg"), ("opus", "opus", "audio/ogg"),
                ):
                    for clip_spec in (None, {"start_seconds": 0.1, "end_seconds": 0.7}):
                        with self.subTest(codec=codec, clip=clip_spec):
                            audio = Path(f"audio-{codec}.{extension}")
                            audio_options = {**options, "audio_codec": codec}
                            media_engine._bounded_output(media_engine._ffmpeg_args(
                                ffmpeg, Path("generated.mp4"), audio, audio_options, "audio", clip_spec,
                            ), 20)
                            info, fmt = media_engine._probe(audio, ffprobe, 10)
                            self.assertEqual({stream["codec_type"] for stream in info["streams"]}, {"audio"})
                            self.assertEqual(media_engine._media_type(info, fmt), mime)
                            if extension == "m4a":
                                self.assertEqual(audio.read_bytes()[4:8], b"ftyp")
            finally:
                os.chdir(old)


if __name__ == "__main__":
    unittest.main()
