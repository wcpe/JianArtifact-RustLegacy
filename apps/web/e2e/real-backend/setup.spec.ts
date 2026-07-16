import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';

interface Credentials {
  username: string;
  password: string;
}

const currentDir = dirname(fileURLToPath(import.meta.url));
const runtimeDir = resolve(currentDir, '../../../../.tmp/e2e-real-backend');

function readCredentials(): Credentials {
  return JSON.parse(readFileSync(resolve(runtimeDir, 'credentials.json'), 'utf8')) as Credentials;
}

test('使用真实一次性令牌通过页面创建首名管理员', async ({ page }) => {
  const credentials = readCredentials();
  const setupToken = readFileSync(resolve(runtimeDir, 'initial-admin.token'), 'utf8').trim();

  await page.addInitScript((token) => {
    window.history.replaceState(null, '', `/setup#token=${encodeURIComponent(token)}`);
  }, setupToken);
  await page.goto('/setup');
  await expect(page).toHaveURL(/\/setup$/);
  await page.fill('#setup-username', credentials.username);
  await page.fill('#setup-password', credentials.password);
  await page.fill('#setup-confirm-password', credentials.password);
  await page.getByRole('button', { name: '创建管理员' }).click();

  await expect(page.getByRole('heading', { name: '登录 JianArtifact' })).toBeVisible();
});
