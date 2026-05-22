# OpenSpec + Superpowers 融合工作流

## 一句话结论

OpenSpec 负责“把需求和变更沉淀成可追踪的文件”，Superpowers 负责“让 Codex 用更稳的工程纪律把它实现出来”。

可以把它理解成：

```text
OpenSpec     = 规格与变更账本
Superpowers  = 讨论、计划、TDD、调试、评审、验证纪律
Codex        = 实际执行者
```

这套融合方案不是再造一个大框架，而是一个薄协调层：在合适阶段调用现有 OpenSpec 命令和 Superpowers skills。

## 你需要知道的 Superpowers 核心能力

如果你之前很少接触 Superpowers，只记住这几个就够用了：

| Superpowers skill                                     | 什么时候用                     | 在融合流程里的作用                           |
| ----------------------------------------------------- | ------------------------------ | -------------------------------------------- |
| `brainstorming`                                       | 需求还不清楚、方案有多种可能   | 先聊清楚目标、约束、取舍，再进入 OpenSpec    |
| `writing-plans`                                       | 已有 spec，但任务还不够细      | 把 OpenSpec 的任务拆成能执行、能测试的小步骤 |
| `test-driven-development`                             | 新功能、bugfix、重构、行为变更 | 先写失败测试，再写实现                       |
| `systematic-debugging`                                | 测试失败、bug、异常行为        | 先找根因，再修复                             |
| `requesting-code-review`                              | 大改动、风险高、准备合并前     | 检查实现是否偏离计划和代码质量要求           |
| `verification-before-completion`                      | 准备说“完成了”之前             | 必须跑测试、读输出、给证据                   |
| `using-git-worktrees` / `subagent-driven-development` | 多任务并行或风险隔离           | 用独立 worktree/subagent 执行，减少互相污染  |

## 默认流程

```text
1. 需求进入
2. 若需求模糊，先用 Superpowers brainstorming 梳理
3. 用 OpenSpec /opsx:explore 或 /opsx:propose 生成变更 artifacts
4. 审阅 proposal.md / design.md / specs/ / tasks.md
5. 若 tasks 太粗，用 Superpowers writing-plans 补成更细的执行计划
6. 用 /opsx:apply 开始实现
7. 实现中按需要启用 TDD 或 systematic-debugging
8. 跑项目测试与 openspec validate
9. 用 verification-before-completion 做完成前检查
10. 验证通过后用 /opsx:archive 归档
```

## 日常怎么对 Codex 说

最简单的说法：

```text
按 OpenSpec + Superpowers 融合流程做这个需求：给用户登录增加短信验证码。
```

如果你已经知道需求很清楚：

```text
按融合流程直接从 OpenSpec propose 开始：新增订单导出 CSV 功能。
```

如果你还没想清楚：

```text
这个需求还比较模糊，先用 Superpowers brainstorming 帮我梳理，再进入 OpenSpec：我要重做会员权益体系。
```

如果已经有 OpenSpec change：

```text
按融合流程继续实现 openspec/changes/add-order-export，先检查 tasks，再 apply，过程中按 TDD 做。
```

## 案例 1：需求清楚的新功能

场景：你要做“订单导出 CSV”。

你可以说：

```text
按 OpenSpec + Superpowers 融合流程做：新增订单导出 CSV 功能，后台用户可以按时间范围导出订单列表。
```

推荐执行路径：

```text
/opsx:propose "add order csv export"
-> 审阅 OpenSpec artifacts
-> /opsx:apply
-> Superpowers test-driven-development
-> openspec validate add-order-csv-export
-> Superpowers verification-before-completion
-> /opsx:archive add-order-csv-export
```

你会得到：

- `openspec/changes/add-order-csv-export/proposal.md`
- `openspec/changes/add-order-csv-export/design.md`
- `openspec/changes/add-order-csv-export/specs/...`
- `openspec/changes/add-order-csv-export/tasks.md`
- 对应代码和测试
- 归档后的长期 spec 更新

## 案例 2：需求很模糊，需要先讨论

场景：你说“我想重做会员权益体系”，但还没决定等级、权益、规则、兼容旧数据。

你可以说：

```text
先按融合流程 brainstorm：我想重做会员权益体系。先帮我厘清方案，不要急着写代码。
```

推荐执行路径：

```text
Superpowers brainstorming
-> 明确目标、用户、规则、边界、迁移风险
-> /opsx:explore 或 /opsx:propose
-> 生成 OpenSpec artifacts
-> 用户确认后再 /opsx:apply
```

这里 Superpowers 的价值是：它会推动 Codex 先问问题、列方案、讲 tradeoff。OpenSpec 的价值是：讨论结果不会只留在聊天里，而是落到 `openspec/changes/<change>/`。

## 案例 3：bugfix，但会影响业务规则

场景：退款金额计算有 bug，修复会改变订单状态、财务口径或接口返回。

你可以说：

```text
按融合流程修复退款金额计算 bug。这个修复可能影响财务口径，请先定位根因，再决定是否创建 OpenSpec change。
```

推荐执行路径：

```text
Superpowers systematic-debugging
-> 复现问题，定位根因
-> 如果只是局部实现错误：TDD 修复 + 验证
-> 如果影响规则/接口/状态：/opsx:propose
-> /opsx:apply
-> openspec validate
-> verification-before-completion
```

判断原则：

- 纯局部 bug：可以不走 OpenSpec，但必须走 debugging、TDD、验证。
- 规则或契约变化：走 OpenSpec，把“新规则”沉淀下来。

## 案例 4：大型重构

场景：要把老的支付模块拆成 provider adapter 架构。

你可以说：

```text
按 OpenSpec + Superpowers 融合流程做支付模块重构。先出方案和迁移计划，不要直接改代码。
```

推荐执行路径：

```text
Superpowers brainstorming
-> /opsx:propose "refactor payment provider adapters"
-> 审阅 design.md 和 specs
-> Superpowers writing-plans，把迁移拆成小任务
-> using-git-worktrees 隔离改动
-> /opsx:apply
-> TDD + code review
-> openspec validate
-> verification-before-completion
-> /opsx:archive
```

大型重构最怕“改到一半忘了为什么这么改”。OpenSpec 负责留下原因和边界，Superpowers 负责让每一步小而可验证。

## 案例 5：已有 OpenSpec change，继续推进

场景：仓库里已经有 `openspec/changes/add-user-auth/`，你想继续实现。

你可以说：

```text
继续 openspec/changes/add-user-auth，按融合流程检查 artifacts 和 tasks，然后开始实现。实现时优先 TDD。
```

推荐执行路径：

```text
openspec status --change add-user-auth
-> 读取 proposal/design/specs/tasks
-> 如果 tasks 足够清楚：/opsx:apply
-> 如果 tasks 太粗：Superpowers writing-plans
-> 实现 + 测试
-> openspec validate add-user-auth
-> verification-before-completion
```

## 什么时候不用完整流程

下面这些可以走轻量流程：

- 改一个文案
- 改一个样式数值
- 修一个很局部、没有规则变化的小 bug
- 调整配置值
- 临时实验或 throwaway prototype

轻量流程也不是“随便改”：至少要检查 diff、跑相关验证，不要在没有证据时说完成。

## 推荐约定

- 所有 OpenSpec artifacts 放在 `openspec/changes/<change-name>/`。
- 不把 Superpowers 的临时设计文档当作长期事实来源；长期规则进 OpenSpec。
- 全局安装 skills，不在每个项目里重复复制 skills。
- 项目首次使用 OpenSpec 时，如果全局 skills 已装好，优先：

```bash
openspec init --tools none --force
```

这样只创建项目本地 `openspec/` artifacts，不重复生成本地 skill。

## 最短上手模板

复制下面这句给 Codex：

```text
按 OpenSpec + Superpowers 融合流程做：<你的需求>。如果需求不清楚，先 brainstorming；如果需求清楚，直接 propose；实现时按 TDD 和 verification-before-completion 执行。
```
