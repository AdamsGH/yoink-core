import unittest
from content_metadata import bounded_metadata, MetadataCollector
from failures import MediaFailure


class NsfwMetadataTests(unittest.TestCase):
    def test_actual_prose_fields_and_legacy_description_cap(self):
        info = {"title": "Fixture sensitivecontent sample", "description": "a" * 1999 + "я" + "sex",
                "tags": ["Эротика"], "categories": ["adult content"],
                "cookies": "not-returned", "url": "not-returned", "is_nsfw": True}
        result = bounded_metadata(info)
        self.assertEqual(result["title"], info["title"])
        self.assertEqual(result["description"], "a" * 1999 + "я")
        self.assertEqual(result["tags"], info["tags"])
        self.assertEqual(result["categories"], info["categories"])
        self.assertEqual(set(result), {"title", "description", "tags", "categories"})

    def test_missing_fields_are_empty_but_bad_types_fail(self):
        self.assertEqual(bounded_metadata({}), {"title": "", "description": "", "tags": [], "categories": []})
        for bad in [{"title": []}, {"title": 5}, {"description": 5}, {"tags": "sex"},
                    {"categories": [5]}, {"tags": ["x"] * 101}, {"title": "x" * 2001}]:
            with self.subTest(bad=bad), self.assertRaises(MediaFailure):
                bounded_metadata(bad)

    def test_batch_retains_later_metadata_and_overflow_fails_not_truncates(self):
        collector = MetadataCollector()
        with self.assertRaises(MediaFailure):
            collector.finish()
        collector.add({"title": "safe"})
        collector.add({"title": "safe"})
        collector.add({"tags": ["sensitivecontent"]})
        self.assertEqual(collector.finish()["title"], "safe")
        self.assertEqual(collector.finish()["tags"], ["sensitivecontent"])
        collector.add({"description": "x" * 2000})
        with self.assertRaises(MediaFailure):
            collector.add({"description": "another source"})
