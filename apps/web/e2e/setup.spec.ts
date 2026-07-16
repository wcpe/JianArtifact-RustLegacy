import { expect, test } from '@playwright/test';
import { DEVMOCK_SETUP_TOKEN } from '@jianartifact/devmock/scenarios';

test('一次性链接创建管理员后可登录', async ({ page }) => {
  await page.addInitScript((token) => {
    window.history.replaceState(null, '', `/setup#token=${encodeURIComponent(token)}`);
  }, DEVMOCK_SETUP_TOKEN);
  await page.goto('/setup');
  await expect(page).toHaveURL(/\/setup$/);
  await expect(page.locator('#setup-username')).toHaveAttribute(
    'autocomplete',
    'section-setup username',
  );
  await expect(page.locator('#setup-password')).toHaveAttribute(
    'autocomplete',
    'section-setup new-password',
  );
  await expect(page.locator('#setup-confirm-password')).toHaveAttribute(
    'autocomplete',
    'section-setup new-password',
  );
  for (const width of [375, 768, 1024]) {
    await page.setViewportSize({ width, height: 900 });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
  }

  await page.fill('#setup-username', 'first-admin');
  await page.fill('#setup-password', 'devmock-secure-password');
  await page.fill('#setup-confirm-password', 'devmock-secure-password');
  await page.getByRole('button', { name: '创建管理员' }).click();

  await expect(page.getByRole('heading', { name: '登录 JianArtifact' })).toBeVisible();
  await page.fill('#login-username', 'first-admin');
  await page.fill('#login-password', 'devmock-secure-password');
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
});
