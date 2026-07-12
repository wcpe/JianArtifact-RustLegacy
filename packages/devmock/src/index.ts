// 包公共出口：向消费方暴露 handlers、双端启动器、场景清单与数据模型公共面。
export { handlers, createHandlers } from './handlers';
export { createDevmockServer } from './node';
export { worker } from './browser';
export { SCENARIOS, isScenarioId, seedFor } from './scenarios';
export type { ScenarioId } from './scenarios';
export { DevmockStore, createStore, switchScenario } from './store';
export type { ApiError, SeedData, StoreResult, TaskAction } from './store';
