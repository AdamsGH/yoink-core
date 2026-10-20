import test from 'node:test';
import assert from 'node:assert/strict';
import { assertPrivateCaption } from './presentation.mjs';

test('presentation assertion rejects bare files and captions without the original linked title', () => {
  const url = 'https://example.com/watch?v=1&lang=en';
  const valid = { caption: '<a href="https://example.com/watch?v=1&amp;lang=en"><b>A &lt;title&gt;</b></a>', parse_mode: 'HTML', caption_above: true };
  assertPrivateCaption(valid, url);
  for (const patch of [
    { caption: undefined }, { parse_mode: undefined }, { caption_above: false },
    { caption: '<b>Title without a link</b>' },
    { caption: valid.caption.replace('A &lt;title&gt;', 'source') },
    { caption: valid.caption.replace('A &lt;title&gt;', '<i>Injected</i>') },
  ]) assert.throws(() => assertPrivateCaption({ ...valid, ...patch }, url));
});
