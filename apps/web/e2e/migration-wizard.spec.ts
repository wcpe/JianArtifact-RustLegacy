// E2E：Nexus 迁移向导全流程（源配置→发现→计划→执行→进度→报告）。
// 对 devmock default 场景：native-dir 来源，任务经轮询推进到 succeeded。
import { expect, test } from '@playwright/test';
import { login } from './helpers';

test('迁移向导从源配置走到报告', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: '迁移' }).click();
  await expect(page.getByRole('heading', { name: 'Nexus 迁移' })).toBeVisible();

  // 步骤一 源配置：默认 native-dir，填数据目录路径（config_source 默认 rest）
  await page.fill('#source-path', '/srv/nexus/data');
  await page.getByRole('button', { name: '开始发现' }).click();

  // 步骤二 发现：默认全选，生成计划
  await expect(page.getByRole('button', { name: '生成计划' })).toBeEnabled();
  await page.getByRole('button', { name: '生成计划' }).click();

  // 步骤三 计划：default 场景无阻断冲突，开始迁移
  await expect(page.getByRole('button', { name: '开始迁移' })).toBeEnabled();
  await page.getByRole('button', { name: '开始迁移' }).click();

  // 步骤四 进度：轮询推进到终态后「查看报告」启用
  const viewReport = page.getByRole('button', { name: '查看报告' });
  await expect(viewReport).toBeEnabled({ timeout: 20_000 });
  await viewReport.click();

  // 步骤五 报告：展示迁移报告标题区
  await expect(page.getByRole('heading', { name: '迁移报告' })).toBeVisible();
});

test('刷新页面在进度阶段可找回任务', async ({ page }) => {
  await login(page);
  await page.goto('/migration');
  await page.fill('#source-path', '/srv/nexus/data');
  await page.getByRole('button', { name: '开始发现' }).click();
  await page.getByRole('button', { name: '生成计划' }).click();
  await page.getByRole('button', { name: '开始迁移' }).click();
  // 进入进度后刷新：task_id 存于 localStorage，重新加载直接回到进度/报告
  await expect(page.getByRole('button', { name: '重新开始' })).toBeVisible();
  await page.reload();
  // 找回后不应回到源配置步（源路径输入框不存在）
  await expect(page.locator('#source-path')).toBeHidden();
  await expect(page.getByRole('button', { name: '重新开始' })).toBeVisible();
});
