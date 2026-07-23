#!/usr/bin/env node
// 受控本地上游：仅提供仓库内 fixture，不访问公网，并记录逐路径及方法请求次数。
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
const methodCounts = new Map();
const artifactWaiters = new Set();
let enabled = true;
let artifactHeld = false;

const largeBlockBytes = 8 * 1024 * 1024;
const blockPatternStride = 131;
const proxySeed = 0x22;
const migrationSeed = 0x33;
const largeSamples = new Map([
  ['1g', { bytes: sampleBytes('ACCEPTANCE_PERFORMANCE_1G_BYTES', 1024 * 1024 * 1024), sha256: null }],
  ['5g', { bytes: sampleBytes('ACCEPTANCE_PERFORMANCE_5G_BYTES', 5 * 1024 * 1024 * 1024), sha256: null }],
]);

function sampleBytes(name, fallback) {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isSafeInteger(value) || value <= 0) throw new Error(`${name} 必须是正整数`);
  return value;
}

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
  const samples = Object.fromEntries(
    [...largeSamples].map(([name, sample]) => [name, { bytes: sample.bytes, configured: sample.sha256 !== null }]),
  );
  return Buffer.from(JSON.stringify({
    enabled,
    total: [...counts.values()].reduce((a, b) => a + b, 0),
    counts: Object.fromEntries(counts),
    method_counts: Object.fromEntries(methodCounts),
    artifact_hold: artifactHeld,
    artifact_waiting: artifactWaiters.size,
    large_samples: samples,
  }));
}

function respond(response, status, body = Buffer.alloc(0), contentType = 'application/octet-stream', method = 'GET') {
  response.writeHead(status, { 'content-type': contentType, 'content-length': body.length });
  response.end(method === 'HEAD' ? undefined : body);
}

function configureLargeSamples(url) {
  for (const name of ['1g', '5g']) {
    const sample = largeSamples.get(name);
    const bytes = Number(url.searchParams.get(`${name}_bytes`));
    const sha256 = url.searchParams.get(`${name}_sha256`) ?? '';
    if (bytes !== sample.bytes || !/^[0-9a-f]{64}$/.test(sha256)) return false;
    sample.sha256 = sha256;
  }
  return true;
}

function releaseArtifactResponses() {
  artifactHeld = false;
  for (const waiter of [...artifactWaiters]) {
    artifactWaiters.delete(waiter);
    if (!waiter.response.destroyed) respond(waiter.response, 200, waiter.body, waiter.contentType, waiter.method);
  }
}

function holdArtifactResponse(response, body, contentType, method) {
  const waiter = { response, body, contentType, method };
  artifactWaiters.add(waiter);
  response.once('close', () => artifactWaiters.delete(waiter));
}

function control(request, response, url) {
  const { pathname } = url;
  if (pathname === '/__acceptance/state' && request.method === 'GET') {
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/reset' && request.method === 'POST') {
    releaseArtifactResponses();
    counts.clear();
    methodCounts.clear();
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/artifact/hold' && request.method === 'POST') {
    artifactHeld = true;
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/artifact/release' && request.method === 'POST') {
    releaseArtifactResponses();
    respond(response, 200, stateBody(), 'application/json', request.method);
    return true;
  }
  if (pathname === '/__acceptance/configure-large' && request.method === 'POST') {
    const valid = configureLargeSamples(url);
    respond(response, valid ? 200 : 400, stateBody(), 'application/json', request.method);
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

function largeSampleForPath(pathname) {
  const direct = pathname.match(/^\/raw\/large-(1g|5g)\.bin$/);
  if (direct) return { sample: largeSamples.get(direct[1]), seed: proxySeed };
  const migration = pathname.match(/^\/migration-(1g|5g)\/repository\/performance-migration-source-\1\/large-\1\.bin$/);
  return migration ? { sample: largeSamples.get(migration[1]), seed: migrationSeed } : undefined;
}

function blockFillByte(seed, blockIndex) {
  return ((seed + blockIndex * blockPatternStride) % 255) + 1;
}

function respondLarge(response, profile, method) {
  const { sample, seed } = profile;
  response.writeHead(200, { 'content-type': 'application/octet-stream', 'content-length': sample.bytes });
  if (method === 'HEAD') {
    response.end();
    return;
  }
  let remaining = sample.bytes;
  let blockIndex = 0;
  const write = () => {
    if (remaining === 0 || response.destroyed) {
      if (!response.destroyed) response.end();
      return;
    }
    const chunkBytes = Math.min(largeBlockBytes, remaining);
    const chunk = Buffer.alloc(chunkBytes, blockFillByte(seed, blockIndex));
    remaining -= chunkBytes;
    blockIndex += 1;
    if (!response.write(chunk)) response.once('drain', write);
    else setImmediate(write);
  };
  write();
}

function nexusResponse(url) {
  const matched = url.pathname.match(/^\/migration-(1g|5g)\/service\/rest\/v1\/(repositories|components)$/);
  if (!matched) return undefined;
  const [label, resource] = matched.slice(1);
  const repository = `performance-migration-source-${label}`;
  if (resource === 'repositories') {
    return [Buffer.from(JSON.stringify([{ name: repository, format: 'raw', type: 'hosted' }])), 200];
  }
  const sample = largeSamples.get(label);
  if (sample.sha256 === null || url.searchParams.get('repository') !== repository) {
    return [Buffer.from(JSON.stringify({ items: [], continuationToken: null })), 503];
  }
  const asset = {
    id: `asset-${label}`,
    downloadUrl: `${origin}/migration-${label}/repository/${repository}/large-${label}.bin`,
    path: `large-${label}.bin`,
    fileSize: sample.bytes,
    checksum: { sha256: sample.sha256 },
  };
  const component = { id: `component-${label}`, name: `large-${label}`, assets: [asset] };
  return [Buffer.from(JSON.stringify({ items: [component], continuationToken: null })), 200];
}

function staticResponse(pathname) {
  const path = resolve(root, `.${decodeURIComponent(pathname)}`);
  if (!path.startsWith(`${root}${sep}`) || !statSync(path, { throwIfNoEntry: false })?.isFile()) return undefined;
  return [readFileSync(path), contentTypes.get(extname(path)) ?? 'application/octet-stream'];
}

const server = createServer((request, response) => {
  const url = new URL(request.url, origin);
  const { pathname } = url;
  if (control(request, response, url)) return;
  counts.set(pathname, (counts.get(pathname) ?? 0) + 1);
  const methodPath = `${request.method} ${pathname}`;
  methodCounts.set(methodPath, (methodCounts.get(methodPath) ?? 0) + 1);
  if (!enabled) {
    respond(response, 503, Buffer.from('受控上游已断开'), 'text/plain; charset=utf-8', request.method);
    return;
  }
  const largeSample = largeSampleForPath(pathname);
  if (largeSample) {
    respondLarge(response, largeSample, request.method);
    return;
  }
  const nexus = nexusResponse(url);
  if (nexus) {
    respond(response, nexus[1], nexus[0], 'application/json', request.method);
    return;
  }
  const result = npmResponse(pathname) ?? staticResponse(pathname);
  if (!result) {
    respond(response, 404, Buffer.alloc(0), 'application/octet-stream', request.method);
    return;
  }
  if (pathname === '/raw/artifact.txt' && artifactHeld) {
    holdArtifactResponse(response, result[0], result[1], request.method);
    return;
  }
  respond(response, 200, result[0], result[1], request.method);
});

server.listen(port, '127.0.0.1', () => {
  writeFileSync(readyPath, JSON.stringify({ port, origin }), { mode: 0o600 });
});
