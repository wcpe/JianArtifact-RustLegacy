// 为真实后端 E2E 创建隔离数据目录和一次性凭据，然后启动 Rust 服务。
import { randomBytes } from 'node:crypto';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import process from 'node:process';

const currentDir = dirname(fileURLToPath(import.meta.url));
const workspaceRoot = resolve(currentDir, '../../../..');
const runtimeDir = resolve(workspaceRoot, '.tmp/e2e-real-backend');
const databasePath = resolve(runtimeDir, 'jianartifact.db');
const dataDir = resolve(runtimeDir, 'blobs');
const credentialsPath = resolve(runtimeDir, 'credentials.json');
const username = `e2e-admin-${randomBytes(4).toString('hex')}`;
const password = randomBytes(24).toString('base64url');

rmSync(runtimeDir, { recursive: true, force: true });
mkdirSync(runtimeDir, { recursive: true });
writeFileSync(credentialsPath, JSON.stringify({ username, password }), 'utf8');

const build = spawnSync(
  'cargo',
  ['build', '--quiet', '-p', 'jianartifact-server', '--bin', 'jianartifact'],
  { cwd: workspaceRoot, stdio: 'inherit' },
);
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);

const binary = resolve(
  workspaceRoot,
  'target/debug',
  process.platform === 'win32' ? 'jianartifact.exe' : 'jianartifact',
);
const child = spawn(binary, [], {
  cwd: workspaceRoot,
  env: {
    ...process.env,
    JIANARTIFACT_BIND: '127.0.0.1:18080',
    JIANARTIFACT_DATABASE_PATH: databasePath,
    JIANARTIFACT_DATA_DIR: dataDir,
    JIANARTIFACT_LOG_LEVEL: 'warn',
    JIANARTIFACT_JWT_SECRET: randomBytes(32).toString('base64url'),
  },
  stdio: 'inherit',
});

function stop(signal) {
  if (child.exitCode === null) child.kill(signal);
}

process.on('SIGINT', () => stop('SIGINT'));
process.on('SIGTERM', () => stop('SIGTERM'));
child.on('exit', (code) => {
  process.exitCode = code ?? 1;
});
