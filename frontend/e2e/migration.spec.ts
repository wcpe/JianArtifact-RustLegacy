import { test, expect } from '@playwright/test';

// Nexus 迁移向导 E2E（FR-142，ADR-0035/0036，Mock 模式）：
// 登录后走统一向导全流程——源配置 → 离线索引双通道加载 → 勾选（含推断补填）→ 执行计划确认 →
// 执行进度 / 报告；并验证执行发起后经任务中心恢复进度视图。
// Mock 后端有状态：离线 index 返 repo_index 双通道样本；搬运 job 首次读取后自动推进到 done。

/** 登录 admin 并落地仪表盘。 */
async function loginAdmin(page: import('@playwright/test').Page) {
  await page.goto('/login');
  await page.getByLabel('用户名').fill('admin');
  await page.getByLabel('口令').fill('admin123');
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
}

test.describe('Nexus 迁移向导（Mock 模式）', () => {
  test('离线全流程：索引双通道 → 勾选（推断补填）→ 计划确认 → 执行 → 报告', async ({ page }) => {
    await loginAdmin(page);

    // 经应用内导航进迁移页（避免整页刷新重置 Mock store）。
    await page.locator('[aria-label="Nexus 迁移"]').click();
    await expect(page).toHaveURL(/\/migration$/);
    await expect(page.getByRole('heading', { name: 'Nexus 迁移' })).toBeVisible();

    // ① 源配置：切到离线，填本地路径，加载仓库索引（双通道）。
    await page.getByText('离线（blob store）').click();
    await page.getByPlaceholder('/data/nexus/blobs/default').fill('/data/blobs');
    await page.getByRole('button', { name: '加载仓库索引' }).click();

    // 索引双通道结果：权威 maven-central + 推断 raw-hosted。
    await expect(page.getByText('maven-central')).toBeVisible();
    await expect(page.getByText('raw-hosted')).toBeVisible();
    await expect(page.getByText('权威').first()).toBeVisible();
    await expect(page.getByText('推断').first()).toBeVisible();

    // ② 勾选：进入勾选步骤，选权威 proxy 项（类型只读、无需补填）。
    await page.getByRole('button', { name: '下一步：勾选仓库' }).click();
    await page.getByRole('checkbox', { name: /maven-central/ }).check();

    // ③ 执行计划：进入并核对方式为 proxy 搬运。
    await page.getByRole('button', { name: '下一步：确认计划' }).click();
    await expect(page.getByText('执行计划', { exact: true })).toBeVisible();
    await expect(page.getByText('proxy 搬运')).toBeVisible();

    // ④ 执行：开始迁移，进入进度视图，随后自动推进至完成报告。
    await page.getByRole('button', { name: '开始迁移' }).click();
    await expect(page.getByText('离线 proxy 搬运')).toBeVisible();
    await expect(page.getByText('已完成').first()).toBeVisible({ timeout: 10000 });
    await expect(page.getByText(/总数/)).toBeVisible();
  });

  test('推断项未补填类型时不可进入执行计划', async ({ page }) => {
    await loginAdmin(page);
    await page.locator('[aria-label="Nexus 迁移"]').click();
    await expect(page).toHaveURL(/\/migration$/);

    await page.getByText('离线（blob store）').click();
    await page.getByPlaceholder('/data/nexus/blobs/default').fill('/data/blobs');
    await page.getByRole('button', { name: '加载仓库索引' }).click();
    await expect(page.getByText('raw-hosted')).toBeVisible();

    await page.getByRole('button', { name: '下一步：勾选仓库' }).click();
    // 勾选推断项 raw-hosted（尚未补填类型）→ 下一步禁用。
    await page.getByRole('checkbox', { name: /raw-hosted/ }).check();
    await expect(page.getByRole('button', { name: '下一步：确认计划' })).toBeDisabled();
    await expect(page.getByText('有勾选的推断仓库尚未确认类型，请先补填后再继续。')).toBeVisible();
  });

  test('执行发起后经任务中心恢复到进度视图（执行中续看）', async ({ page }) => {
    await loginAdmin(page);
    await page.locator('[aria-label="Nexus 迁移"]').click();
    await expect(page).toHaveURL(/\/migration$/);

    // 在线预览 → 勾选 → 计划 → 执行，发起后本页存档 job_id。
    await page.getByPlaceholder('https://nexus.example').first().fill('https://nexus.example');
    await page.getByRole('button', { name: '预览仓库' }).click();
    await expect(page.getByText('maven-internal')).toBeVisible();
    await page.getByRole('button', { name: '下一步：勾选仓库' }).click();
    await page.getByRole('checkbox', { name: /maven-internal/ }).check();
    await page.getByRole('button', { name: '下一步：确认计划' }).click();
    await page.getByRole('button', { name: '开始迁移' }).click();
    await expect(page.getByText('在线拉取导入队列')).toBeVisible();

    // 经任务中心（应用内导航，保留 store）跳回该迁移任务续看进度。
    await page.locator('[aria-label="任务中心"]').click();
    await expect(page).toHaveURL(/\/tasks$/);
    // 任务中心列出该迁移任务，点开详情后跳转续看回迁移页。
    await page.getByText('在线拉取导入队列').first().click();
    await page.getByRole('button', { name: '跳转查看' }).click();
    await expect(page).toHaveURL(/\/migration\?job_id=/);
    // 恢复到进度 / 报告视图（Mock 首读已推进至完成）。
    await expect(page.getByText('已完成').first()).toBeVisible({ timeout: 10000 });
  });
});
