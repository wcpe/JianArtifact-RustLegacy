// 真实后端认证闭环：登录、会话恢复、Token、用户管理与修改本人口令。
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';

interface Credentials {
  username: string;
  password: string;
}

const currentDir = dirname(fileURLToPath(import.meta.url));
const credentialsPath = resolve(currentDir, '../../../../.tmp/e2e-real-backend/credentials.json');

function readCredentials(): Credentials {
  return JSON.parse(readFileSync(credentialsPath, 'utf8')) as Credentials;
}

async function login(page: Page, username: string, password: string): Promise<void> {
  await page.goto('/login');
  await expect(page.locator('#login-username')).toHaveAttribute('autocomplete', 'username');
  await expect(page.locator('#login-password')).toHaveAttribute('autocomplete', 'current-password');
  await page.fill('#login-username', username);
  await page.fill('#login-password', password);
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
}

test('真实后端完成认证与管理闭环', async ({ page, request }) => {
  const autocompleteWarnings: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'warning' && /autocomplete/i.test(message.text())) {
      autocompleteWarnings.push(message.text());
    }
  });
  const favicon = await request.get('/favicon.ico');
  expect(favicon.status()).toBe(200);

  const credentials = readCredentials();
  const suffix = randomBytes(4).toString('hex');
  const userName = `e2e-user-${suffix}`;
  const userPassword = randomBytes(18).toString('base64url');
  const newAdminPassword = randomBytes(18).toString('base64url');

  await login(page, credentials.username, credentials.password);
  await page.reload();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
  await expect(page.getByText(credentials.username).first()).toBeVisible();

  await page.goto('/tokens');
  await page.getByRole('button', { name: '新建 Token' }).click();
  await page.fill('#token-name', `real-e2e-${suffix}`);
  await page.getByRole('button', { name: '创建', exact: true }).click();
  await expect(page.getByText('请立即保存 Token 明文（仅此一次）')).toBeVisible();
  await expect(page.getByText(/^jat_/)).toBeVisible();

  await page.goto('/users');
  await page.getByRole('button', { name: '新增用户' }).click();
  await page.fill('#user-username', userName);
  await page.fill('#user-password', userPassword);
  await page.getByRole('button', { name: '创建', exact: true }).click();
  await expect(page.getByRole('cell', { name: userName, exact: true })).toBeVisible();

  await page.getByRole('button', { name: new RegExp(credentials.username) }).click();
  await page.getByRole('menuitem', { name: '修改口令' }).click();
  await expect(page.locator('#password-old')).toHaveAttribute(
    'autocomplete',
    'section-change-password current-password',
  );
  await expect(page.locator('#password-new')).toHaveAttribute(
    'autocomplete',
    'section-change-password new-password',
  );
  await expect(page.locator('#password-confirm')).toHaveAttribute(
    'autocomplete',
    'section-change-password new-password',
  );
  await page.fill('#password-old', credentials.password);
  await page.fill('#password-new', newAdminPassword);
  await page.fill('#password-confirm', newAdminPassword);
  await page.getByRole('button', { name: '确认修改' }).click();
  await expect(page.getByRole('heading', { name: '登录 JianArtifact' })).toBeVisible();

  await page.fill('#login-username', credentials.username);
  await page.fill('#login-password', credentials.password);
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByText('用户名或口令无效')).toBeVisible();

  await login(page, credentials.username, newAdminPassword);
  expect(autocompleteWarnings).toEqual([]);
});
