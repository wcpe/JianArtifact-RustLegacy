// 组件验收站首页：集中预览共享组件库的组件形态，作为验收与回归的可视化基准
import { Button } from '@jianartifact/ui';

// 当前收录的全部按钮 variant 与 size，组合成预览矩阵
const variants = ['default', 'secondary', 'destructive', 'outline', 'ghost', 'link'] as const;
const sizes = ['sm', 'default', 'lg'] as const;

export default function App() {
  return (
    <main className="mx-auto flex max-w-4xl flex-col gap-8 p-8">
      <h1 className="text-3xl font-bold">JianArtifact 组件验收站</h1>

      {/* 组件索引：随组件库扩充逐项登记 */}
      <section className="flex flex-col gap-2">
        <h2 className="text-xl font-semibold">组件索引</h2>
        <ul className="list-inside list-disc text-muted-foreground">
          <li>Button 按钮</li>
        </ul>
      </section>

      {/* Button 预览区：variant × size 矩阵与禁用态 */}
      <section className="flex flex-col gap-4">
        <h2 className="text-xl font-semibold">Button 按钮</h2>

        <h3 className="text-sm font-medium text-muted-foreground">正常态</h3>
        <div className="flex flex-col gap-3">
          {variants.map((variant) => (
            <div key={variant} className="flex flex-wrap items-center gap-3">
              {sizes.map((size) => (
                <Button key={size} variant={variant} size={size}>
                  {variant} / {size}
                </Button>
              ))}
            </div>
          ))}
        </div>

        <h3 className="text-sm font-medium text-muted-foreground">禁用态</h3>
        <div className="flex flex-wrap items-center gap-3">
          {variants.map((variant) => (
            <Button key={variant} variant={variant} disabled>
              {variant}
            </Button>
          ))}
        </div>
      </section>
    </main>
  );
}
