import assert from 'node:assert/strict';

export function isoBmffMdatPayloads(bytes) {
  assert(bytes.length <= 2 * 1024 * 1024, 'ISO-BMFF fixture exceeds parser bound');
  const payloads = [];
  let offset = 0;
  let firstBox = true;
  while (offset < bytes.length) {
    assert(bytes.length - offset >= 8, 'truncated ISO-BMFF box header');
    const size32 = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    let headerSize = 8;
    let boxSize;
    if (size32 === 1) {
      assert(bytes.length - offset >= 16, 'truncated extended ISO-BMFF box header');
      const extended = bytes.readBigUInt64BE(offset + 8);
      assert(extended <= BigInt(Number.MAX_SAFE_INTEGER), 'extended ISO-BMFF box size exceeds safe integer range');
      boxSize = Number(extended);
      headerSize = 16;
    } else if (size32 === 0) {
      boxSize = bytes.length - offset;
    } else {
      boxSize = size32;
    }
    assert(boxSize >= headerSize && boxSize <= bytes.length - offset, 'invalid or truncated ISO-BMFF box size');
    if (firstBox) {
      assert.equal(type, 'ftyp', 'ISO-BMFF media must begin with ftyp');
      assert(boxSize >= headerSize + 8, 'truncated ftyp box payload');
      firstBox = false;
    }
    if (type === 'mdat') payloads.push(bytes.subarray(offset + headerSize, offset + boxSize));
    offset += boxSize;
  }
  assert.equal(offset, bytes.length, 'ISO-BMFF boxes must frame the complete media buffer');
  return payloads;
}
