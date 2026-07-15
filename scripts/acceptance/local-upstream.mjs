#!/usr/bin/env node
// 受控本地上游：仅提供仓库内 fixture，不访问公网，并记录逐路径请求次数。
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { readFileSync, statSync, writeFileSync } from 'node:fs';
import { basename, extname, resolve, sep } from 'node:path';
import { gzipSync } from 'node:zlib';

const args = Object.fromEntries(
  process.argv.slice(2).map((value) => {
    const [key, content] = value.split('=', 2);
    return [key.replace(/^--/, ''), content];
  }),
);
const root = resolve(args.root);
const port = Number(args.port);
const readyPath = resolve(args.ready);
const origin = `http://127.0.0.1:${port}`;
const contentTypes = new Map([
  ['.json', 'application/json'],
  ['.pom', 'application/xml'],
  ['.xml', 'application/xml'],
  ['.tgz', 'application/octet-stream'],
]);
const counts = new Map();
let enabled = true;

function tarHeader(name, size, mode) {
  const header = Buffer.alloc(512);
  const write = (value, offset, length) => header.write(value, offset, Math.min(length, Buffer.byteLength(value)), 'ascii');
  const octal = (value, length) => `${value.toString(8).padStart(length - 1, '0')}\0`;
  write(name, 0, 100);
  write(octal(mode, 8), 100, 8);
  write(octal(0, 8), 108, 8);
  write(octal(0, 8), 116, 8);
  write(octal(size, 12), 124, 12);
  write(octal(0, 12), 136, 12);
  header.fill(0x20, 148, 156);
  header[156] = 0x30;
  write('ustar\0', 257, 6);
  write('00', 263, 2);
  write(octal([...header].reduce((sum, byte) => sum + byte, 0), 8), 148, 8);
  return header;
}

function packageTarball(directory) {
  const entries = ['package.json', 'index.js'].map((name) => {
    const body = readFileSync(resolve(directory, name));
    const padding = Buffer.alloc((512 - (body.length % 512)) % 512);
    return Buffer.concat([tarHeader(`package/${name}`, body.length, 0o644), body, padding]);
  });
  return gzipSync(Buffer.concat([...entries, Buffer.alloc(1024)]), { mtime: 0 });
}

function npmFixture(name) {
  const directory = resolve(root, 'npm', 'packages', name);
  const manifest = JSON.parse(readFileSync(resolve(directory, 'package.json'), 'utf8'));
  const tarball = packageTarball(directory);
  const filename = `${name}-${manifest.version}.tgz`;
  const packument = {
    name,
    'dist-tags': { latest: manifest.version },
    versions: {
      [manifest.version]: {
        ...manifest,
        dist: {
          tarball: `${origin}/npm/${name}/-/${filename}`,
          integrity: `sha512-${createHash('sha512').update(tarball).digest('base64')}`,
          shasum: createHash('sha1').update(tarball).digest('hex'),
        },
      },
    },
  };
  return { filename, packument: Buffer.from(JSON.stringify(packument)), tarball };
}

const npmFixtures = new Map(
  ['upstream-only', 'group-only', 'group-conflict'].map((name) => [name, npmFixture(name)]),
);

function stateBody() {
  return Buffer.from(JSON.stringify({ enabled, total: [...counts.values()].reduce((a, b) => a + b, 0), counts: Object.fromEntries(counts) }));
}

function respond(response, status, body = Buffer.alloc(0), contentType = 'application/octet-stream', method = 'GET') {
  response.writeHead(status, { 'content-type': contentType, 'content-length': body.length });
  response.end(method === 'HEAD' ? undefined : body);
}

function control(request, response, pathname) {
  if (pathname === '/__acceptance/state' && request.method === 'GET') {
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/reset' && request.method === 'POST') {
    counts.clear();
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/enable' && request.method === 'POST') {
    enabled = true;
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/disable' && request.method === 'POST') {
    enabled = false;
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  return false;
}

function npmResponse(pathname) {
  for (const [name, fixture] of npmFixtures) {
    if (pathname === `/npm/${name}`) return [fixture.packument, 'application/json'];
    if (pathname === `/npm/${name}/-/${fixture.filename}`) return [fixture.tarball, 'application/octet-stream'];
  }
  return undefined;
}

function staticResponse(pathname) {
  const path = resolve(root, `.${decodeURIComponent(pathname)}`);
  if (!path.startsWith(`${root}${sep}`) || !statSync(path, { throwIfNoEntry: false })?.isFile()) return undefined;
  return [readFileSync(path), contentTypes.get(extname(path)) ?? 'application/octet-stream'];
}

const server = createServer((request, response) => {
  const pathname = new URL(request.url, origin).pathname;
  if (control(request, response, pathname)) return;
  counts.set(pathname, (counts.get(pathname) ?? 0) + 1);
  if (!enabled) {
    respond(response, 503, Buffer.from('受控上游已断开'), 'text/plain; charset=utf-8', request.method);
    return;
  }
  const result = npmResponse(pathname) ?? staticResponse(pathname);
  if (!result) {
    respond(response, 404, Buffer.alloc(0), 'application/octet-stream', request.method);
    return;
  }
  respond(response, 200, result[0], result[1], request.method);
});

server.listen(port, '127.0.0.1', () => {
  writeFileSync(readyPath, JSON.stringify({ port, origin }), { mode: 0o600 });
});
