"""Actual HTML, GitHub and YouTube extraction, without API-process networking."""
import base64
from html import unescape
from html.parser import HTMLParser
import json
import re
from urllib.parse import quote, urlencode, urlsplit, parse_qs
import xml.etree.ElementTree as ET
from yt_dlp.utils import DownloadError

from content_transport import ContentError
import network_guard
from youtube_captions import caption_tracks


class HTMLText(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack = []
        self.all = []
        self.main = []
        self.title = []

    def handle_starttag(self, tag, attrs):
        if tag not in ('br', 'hr', 'img', 'meta', 'link', 'input', 'source', 'wbr', 'area', 'base', 'embed', 'param', 'track', 'col'):
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if tag in self.stack:
            self.stack = self.stack[:len(self.stack) - 1 - self.stack[::-1].index(tag)]

    def handle_data(self, data):
        if 'title' in self.stack:
            self.title.append(data.strip())
        if any(t in self.stack for t in ('script', 'style', 'nav', 'header', 'footer', 'aside', 'form', 'iframe', 'head')):
            return
        if data.strip():
            self.all.append(data.strip())
            if 'main' in self.stack or 'article' in self.stack:
                self.main.append(data.strip())


def html_extract(html, url):
    parser = HTMLText()
    parser.feed(html)
    title = ' '.join(parser.title) or None
    # Optional high-quality legacy extractors, all purely local.
    try:
        from readability import Document
        doc = Document(html, url=url)
        summary = HTMLText()
        summary.feed(doc.summary())
        text = '\n'.join(summary.all)
        if len(text) > 200:
            return text, doc.title() or title, 'readability'
    except Exception:
        pass
    if parser.main:
        return '\n'.join(parser.main), title, 'html-main'
    try:
        import trafilatura
        text = trafilatura.extract(html, include_comments=False, include_tables=True)
        if text and len(text) > 200:
            return text, title, 'trafilatura'
    except Exception:
        pass
    return '\n'.join(parser.all), title, 'raw-html'


def optional(call):
    try:
        return call()
    except (network_guard.NetworkPolicyError, network_guard.NetworkValidationError):
        raise
    except ContentError as e:
        if e.code in ('content_too_large', 'content_timeout', 'content_invalid_response'):
            raise
        return None
    except (OSError, ValueError):
        return None


def github_route(url):
    p = urlsplit(url)
    parts = p.path.strip('/').split('/')
    if p.hostname == 'api.github.com':
        return url, 'api'
    if p.hostname == 'raw.githubusercontent.com':
        return url, 'raw'
    if p.hostname != 'github.com' or len(parts) < 2:
        return None
    owner, repo, *rest = parts
    if not all(re.fullmatch(r'[A-Za-z0-9_.-]+', s) for s in (owner, repo)):
        return None
    base = 'https://api.github.com/repos/' + owner + '/' + repo
    if not rest:
        return base, 'repo'
    section, *tail = rest
    if section == 'blob' and len(tail) >= 2:
        return base + '/contents/' + '/'.join(quote(s, safe='') for s in tail[1:]) + '?' + urlencode({'ref': tail[0]}), 'file'
    if section == 'tree':
        return base + '/git/trees/' + quote(tail[0] if tail else 'HEAD', safe='') + '?recursive=1', 'tree'
    sections = {'issues': 'issues', 'pull': 'pulls', 'pulls': 'pulls', 'commits': 'commits', 'commit': 'commits', 'releases': 'releases'}
    if section in sections:
        suffix = '/' + sections[section]
        if tail:
            suffix += '/' + '/'.join(quote(s, safe='') for s in tail)
        else:
            suffix += '?per_page=30'
        return base + suffix, section
    return base, 'repo'


def github_fetch(transport, route, token):
    url, op = route
    if op == 'raw':
        return transport.get(url), None, 'github:raw'
    if op == 'file':
        p = urlsplit(url)
        path = p.path.split('/')
        ref = parse_qs(p.query).get('ref', ['HEAD'])[0]
        raw_url = 'https://raw.githubusercontent.com/' + path[2] + '/' + path[3] + '/' + quote(ref, safe='') + '/' + '/'.join(path[5:])
        raw = optional(lambda: transport.get(raw_url))
        if raw and raw.strip():
            return raw, None, 'github:raw'
    data = transport.json(url, token)
    title = data.get('title') or data.get('full_name') if isinstance(data, dict) else None
    if op == 'file' and isinstance(data, dict) and data.get('encoding') == 'base64':
        try:
            text = base64.b64decode(data['content'], validate=False).decode('utf-8', 'replace')
        except (KeyError, ValueError):
            raise ContentError('content_invalid_response') from None
    else:
        text = json.dumps(data, ensure_ascii=False, indent=2)
    if op == 'repo':
        readme = optional(lambda: transport.json(url + '/readme', token))
        if isinstance(readme, dict) and readme.get('encoding') == 'base64':
            text += '\n\n## README\n' + base64.b64decode(readme['content']).decode('utf-8', 'replace')
    if op in ('issues', 'pull', 'pulls') and isinstance(data, dict) and data.get('number'):
        comments = optional(lambda: transport.json(url + '/comments?per_page=20', token))
        if comments:
            text += '\n\n## Comments\n' + json.dumps(comments, ensure_ascii=False)
    return text, title, 'github-api:' + op


def video_id(url):
    p = urlsplit(url)
    if p.hostname == 'youtu.be':
        value = p.path.strip('/').split('/')[0]
    elif p.hostname in ('youtube.com', 'www.youtube.com', 'm.youtube.com', 'music.youtube.com'):
        parts = p.path.strip('/').split('/')
        value = parse_qs(p.query).get('v', [''])[0]
        if not value and len(parts) == 2 and parts[0] in ('shorts', 'embed', 'v'):
            value = parts[1]
    else:
        return None
    return value if re.fullmatch(r'[A-Za-z0-9_-]{11}', value) else None


def player_response(html):
    for match in re.finditer(r'(?:ytInitialPlayerResponse\s*=|"ytInitialPlayerResponse"\s*:)\s*', html):
        try:
            value, _ = json.JSONDecoder().raw_decode(html[match.end():])
            if isinstance(value, dict):
                return value
        except ValueError:
            continue
    raise ContentError('content_invalid_response')


def youtube_transcript(transport, tracks, languages):
    if not isinstance(tracks, list):
        return None
    tracks = [track for track in tracks if isinstance(track, dict)]
    # Ordered language preference, manual before ASR within each language; then any.
    tracks = sorted(enumerate(tracks), key=lambda it: (languages.index(it[1].get('languageCode')) if it[1].get('languageCode') in languages else len(languages), it[1].get('kind') == 'asr', it[0]))
    for _, track in tracks:
        if not isinstance(track.get('baseUrl'), str):
            continue
        raw = optional(lambda: transport.get(track['baseUrl']))
        if not raw:
            continue
        try:
            if raw.lstrip().startswith('{'):
                events = json.loads(raw).get('events', [])
                text = '\n'.join(''.join(s.get('utf8', '') for s in e.get('segs', [])) for e in events).strip()
            else:
                root = ET.fromstring(raw)
                text = '\n'.join(unescape(''.join(e.itertext())) for e in root.iter() if e.tag in ('text', 'p')).strip()
        except (ValueError, TypeError, AttributeError, ET.ParseError):
            continue
        if text:
            return text, track.get('languageCode', 'unknown')
    return None


def youtube_fetch(transport, vid, languages, transcript_only):
    player = player_response(transport.get('https://www.youtube.com/watch?v=' + vid))
    details = player.get('videoDetails', {})
    title = details.get('title')
    try:
        seconds = int(details.get('lengthSeconds', 0)) or None
        if seconds is not None and not 0 <= seconds <= 2147483647:
            seconds = None
    except (ValueError, TypeError):
        seconds = None
    tracks = player.get('captions', {}).get('playerCaptionsTracklistRenderer', {}).get('captionTracks', [])
    transcript = youtube_transcript(transport, tracks, languages)
    if not transcript and tracks:
        # Empty watch-page captions can require a different player client, not URL edits.
        try:
            alternate = caption_tracks(transport, vid)
        except DownloadError:
            alternate = []
        except ContentError as error:
            if error.code not in ('content_http_error', 'content_invalid_response'):
                raise
            alternate = []
        transcript = youtube_transcript(transport, alternate, languages)
    if transcript:
        return transcript[0], title, seconds, 'youtube-transcript:' + transcript[1]
    if transcript_only:
        raise ContentError('no_transcript')
    description = details.get('shortDescription', '')
    if description:
        return description, title, seconds, 'youtube-description'
    raise ContentError('no_content')


def web_fetch(transport, url, token):
    route = github_route(url)
    if route:
        result = optional(lambda: github_fetch(transport, route, token))
        if result and result[0].strip():
            return result
    p = urlsplit(url)
    matched = None
    if not route:
        llms = optional(lambda: transport.get(p.scheme + '://' + p.netloc + '/llms.txt'))
        if llms:
            for link in re.findall(r'\[[^\]]+\]\((https?://[^)]+)\)', llms):
                lp = urlsplit(link)
                if lp.path.rstrip('/').removesuffix('/index.md').removesuffix('.md') == p.path.rstrip('/'):
                    matched = link
                    break
    html = optional(lambda: transport.get(matched or url))
    raw_result = None
    if html and not route:
        if matched and not re.search(r'<(?:html|body)\b', html, re.I):
            return html, None, 'llms.txt'
        raw_result = html_extract(html, matched or url)
        if raw_result[2] != 'raw-html' and raw_result[0].strip():
            return raw_result[0], raw_result[1], ('llms.txt+' if matched else '') + raw_result[2]
    # Provider fallback is deliberately anonymous, and source URL was validated first.
    for name, base in [('jina', 'https://r.jina.ai/'), ('curl.md', 'https://curl.md/')]:
        text = optional(lambda: transport.get(base + url))
        if text:
            text = re.sub(r'\A---\n.*?\n---\n+', '', text, count=1, flags=re.S)
            text = re.sub(r'\n+---\n+Powered by .*\Z', '', text, flags=re.S)
            if len(re.sub(r'[\s-]', '', text)) >= 100:
                return text.strip(), None, name
    if raw_result and raw_result[0].strip():
        return raw_result
    raise ContentError('no_content')
