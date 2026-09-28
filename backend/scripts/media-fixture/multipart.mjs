import assert from 'node:assert/strict';

export function parseMediaBody(contentType, body) {
  const boundary = contentType.match(/boundary=(?:"([^"]+)"|([^;]+))/)?.slice(1).find(Boolean);
  if (!boundary) {
    assert.match(contentType, /application\/json/);
    return { data: JSON.parse(body.toString('utf8')), files: new Map() };
  }
  const delimiter = Buffer.from(`--${boundary}`);
  const data = {};
  const files = new Map();
  const names = new Set();
  let offset = body.indexOf(delimiter);
  assert.equal(offset, 0, 'multipart must start with its boundary');
  while (offset >= 0) {
    const start = offset + delimiter.length;
    if (body.subarray(start, start + 2).toString() === '--') return { data, files };
    const headersEnd = body.indexOf(Buffer.from('\r\n\r\n'), start);
    assert(headersEnd >= 0, 'multipart part must have headers');
    const next = body.indexOf(delimiter, headersEnd + 4);
    assert(next >= 0, 'multipart part must terminate at a boundary');
    assert.equal(body.subarray(next - 2, next).toString(), '\r\n');
    const headers = body.subarray(start, headersEnd).toString('latin1');
    const name = headers.match(/\bname="([^"]+)"/)?.[1];
    assert(name && !names.has(name), 'multipart names must be present and unique');
    names.add(name);
    const value = body.subarray(headersEnd + 4, next - 2);
    const filename = headers.match(/\bfilename="([^"]+)"/)?.[1];
    if (filename !== undefined) {
      files.set(name, { headers, value });
    } else {
      const text = value.toString('utf8');
      data[name] = text.startsWith('"') ? JSON.parse(text) : text;
    }
    offset = next;
  }
  throw new Error('multipart closing boundary is missing');
}
