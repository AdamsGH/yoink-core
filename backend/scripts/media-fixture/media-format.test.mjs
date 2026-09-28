import test from 'node:test';
import assert from 'node:assert/strict';
import { isoBmffMdatPayloads } from './media-format.mjs';

test('ISO-BMFF parser returns framed media payloads', () => {
  const ftyp = Buffer.from([0, 0, 0, 16, 102, 116, 121, 112, 105, 115, 111, 109, 0, 0, 0, 0]);
  const mdat = Buffer.from([0, 0, 0, 11, 109, 100, 97, 116, 1, 2, 3]);
  assert.deepEqual(isoBmffMdatPayloads(Buffer.concat([ftyp, mdat])), [Buffer.from([1, 2, 3])]);
});

test('ISO-BMFF parser rejects truncated and non-ftyp media', () => {
  assert.throws(() => isoBmffMdatPayloads(Buffer.from([0, 0, 0, 8, 109, 100, 97, 116])), /begin with ftyp/);
  assert.throws(() => isoBmffMdatPayloads(Buffer.from([0, 0, 0])), /truncated ISO-BMFF box header/);
});
