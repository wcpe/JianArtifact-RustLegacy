// 真实 Rust 后端仓库核心闭环：创建、配置、ACL、冲突、404 与确认删除。
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';

interface Credentials {
  username: string;
  password: string;
}

interface CreatedUser {
  id: number;
  username: string;
}

const currentDir = dirname(fileURLToPath(import.meta.url));
const credentialsPath = resolve(currentDir, '../../../../.tmp/e2e-real-backend/credentials.json');

function readCredentials(): Credentials {
  return JSON.parse(readFileSync(credentialsPath, 'utf8')) as Credentials;
}

async function login(page: Page, credentials: Credentials): Promise<void> {
  await page.goto('/login');
  await page.fill('#login-username', credentials.username);
  await page.fill('#login-password', credentials.password);
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
}

async function apiLogin(page: Page, credentials: Credentials): Promise<string> {
  const response = await page.request.post('/api/v1/auth/login', { data: credentials });
  expect(response.status()).toBe(200);
  const body = (await response.json()) as { token: string };
  return body.token;
}

async function createAclUser(
  page: Page,
  token: string,
  username: string,
  password: string,
): Promise<CreatedUser> {
  const response = await page.request.post('/api/v1/users', {
    headers: { Authorization: `Bearer ${token}` },
    data: { username, password, role: 'user' },
  });
  expect(response.status()).toBe(201);
  return (await response.json()) as CreatedUser;
}

function waitForApi(page: Page, method: string, pathname: string) {
  return page.waitForResponse((response) => {
    const url = new URL(response.url());
    return response.request().method() === method && url.pathname === pathname;
  });
}

async function createRepositoryFromUi(page: Page, repoName: string): Promise<void> {
  await page.goto('/repos');
  await expect(page.getByRole('heading', { name: '仓库', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '新建仓库' }).click();
  await page.fill('#repo-name', repoName);
  const responsePromise = waitForApi(page, 'POST', '/api/v1/repos');
  await page.getByRole('button', { name: '创建', exact: true }).click();
  const response = await responsePromise;
  expect(response.status()).toBe(201);
  await expect(page.getByRole('link', { name: repoName, exact: true })).toBeVisible();
  await page.getByRole('link', { name: repoName, exact: true }).click();
  await expect(page.getByRole('heading', { name: repoName })).toBeVisible();
}

async function verifyUsageContract(page: Page, token: string, repoName: string): Promise<void> {
  const response = await page.request.get(`/api/v1/repos/${repoName}/usage`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(response.status()).toBe(200);
  const snippets = (await response.json()) as { title: string; content: string }[];
  expect(snippets.length).toBeGreaterThan(0);
  expect(snippets.every((snippet) => snippet.title && snippet.content)).toBe(true);
  expect(snippets.some((snippet) => snippet.content.includes('<令牌>'))).toBe(true);
}

async function updateRepositoryFromUi(page: Page, repoName: string): Promise<void> {
  await page.getByRole('tab', { name: '配置' }).click();
  await page.getByRole('combobox', { name: '可见性' }).click();
  await page.getByRole('option', { name: '公开' }).click();
  await page.getByRole('combobox', { name: '覆盖策略' }).click();
  await page.getByRole('option', { name: '禁止覆盖' }).click();
  const responsePromise = waitForApi(page, 'PATCH', `/api/v1/repos/${repoName}`);
  await page.getByRole('button', { name: '保存' }).click();
  const response = await responsePromise;
  expect(response.status()).toBe(200);
  const body = (await response.json()) as { visibility: string; overwrite_policy: string };
  expect(body.visibility).toBe('public');
  expect(body.overwrite_policy).toBe('deny');
  await expect(page.getByText('已保存')).toBeVisible();
}

async function manageAclFromUi(page: Page, repoName: string, user: CreatedUser): Promise<void> {
  await page.getByRole('tab', { name: '权限' }).click();
  await expect(page.getByText(/暂无授权/)).toBeVisible();
  await page.fill('#acl-user', user.username);
  const addPromise = waitForApi(page, 'PUT', `/api/v1/repos/${repoName}/acl/${user.username}`);
  await page.getByRole('button', { name: '添加' }).click();
  const added = await addPromise;
  expect(added.status()).toBe(200);

  const row = page
    .getByRole('row')
    .filter({ has: page.getByRole('cell', { name: user.username }) });
  await expect(row.getByRole('cell', { name: String(user.id), exact: true })).toBeVisible();
  await expect(row.getByRole('cell', { name: user.username, exact: true })).toBeVisible();
  await expect(row.getByText('读', { exact: true })).toBeVisible();

  const removePromise = waitForApi(
    page,
    'DELETE',
    `/api/v1/repos/${repoName}/acl/${user.username}`,
  );
  await row.getByRole('button', { name: '移除' }).click();
  expect((await removePromise).status()).toBe(204);
  await expect(page.getByText(/暂无授权/)).toBeVisible();
}

async function verifyDuplicateConflict(page: Page, repoName: string): Promise<void> {
  await page.goto('/repos');
  await page.getByRole('button', { name: '新建仓库' }).click();
  await page.fill('#repo-name', repoName);
  const responsePromise = waitForApi(page, 'POST', '/api/v1/repos');
  await page.getByRole('button', { name: '创建', exact: true }).click();
  expect((await responsePromise).status()).toBe(409);
  await expect(page.getByText('仓库名已存在', { exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
}

async function verifyMissingRepository(page: Page, missingName: string): Promise<void> {
  const responsePromise = waitForApi(page, 'GET', `/api/v1/repos/${missingName}`);
  await page.goto(`/repos/${missingName}`);
  expect((await responsePromise).status()).toBe(404);
  await expect(page.getByText('仓库不存在或无权访问', { exact: true })).toBeVisible();
}

async function deleteRepositoryFromUi(page: Page, repoName: string): Promise<void> {
  await page.goto(`/repos/${repoName}?tab=config`);
  await expect(page.getByRole('heading', { name: repoName })).toBeVisible();
  await page.getByRole('button', { name: '删除仓库', exact: true }).click();
  const confirm = page.getByRole('button', { name: '确认删除' });
  await page.getByLabel(`输入仓库名 ${repoName} 以确认`).fill(`${repoName}-wrong`);
  await expect(confirm).toBeDisabled();
  await page.getByLabel(`输入仓库名 ${repoName} 以确认`).fill(repoName);
  await expect(confirm).toBeEnabled();
  const responsePromise = waitForApi(page, 'DELETE', `/api/v1/repos/${repoName}`);
  await confirm.click();
  const response = await responsePromise;
  expect(response.status()).toBe(200);
  await expect(page).toHaveURL(/\/repos$/);
  await expect(page.getByRole('link', { name: repoName, exact: true })).toBeHidden();
}

test('真实 Rust 后端使用片段契约返回结构化占位内容', async ({ page }) => {
  const credentials = readCredentials();
  const repoName = `e2e-usage-${randomBytes(5).toString('hex')}`;
  await login(page, credentials);
  const token = await apiLogin(page, credentials);

  try {
    await createRepositoryFromUi(page, repoName);
    await verifyUsageContract(page, token, repoName);
  } finally {
    await page.request.delete(`/api/v1/repos/${repoName}?mode=data`, {
      headers: {
        Authorization: `Bearer ${token}`,
        'X-Confirm-Repository': repoName,
      },
    });
  }
});

test('真实 Rust 后端完成仓库核心验收闭环', async ({ page }) => {
  const credentials = readCredentials();
  const suffix = randomBytes(5).toString('hex');
  const repoName = `e2e-repo-${suffix}`;
  const aclUsername = `e2e-acl-${suffix}`;
  const missingName = `e2e-missing-${suffix}`;

  await login(page, credentials);
  const token = await apiLogin(page, credentials);
  const aclUser = await createAclUser(
    page,
    token,
    aclUsername,
    randomBytes(18).toString('base64url'),
  );

  await createRepositoryFromUi(page, repoName);
  await updateRepositoryFromUi(page, repoName);
  await manageAclFromUi(page, repoName, aclUser);
  await verifyDuplicateConflict(page, repoName);
  await verifyMissingRepository(page, missingName);
  await deleteRepositoryFromUi(page, repoName);

  const deleted = await page.request.get(`/api/v1/repos/${repoName}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(deleted.status()).toBe(404);
});
