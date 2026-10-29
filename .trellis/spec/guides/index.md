# Thinking Guides

> **Purpose**: Expand your thinking to catch things you might not have considered.

---

## Why Thinking Guides?

**Most bugs and tech debt come from "didn't think of that"**, not from lack of skill:

- Didn't think about what happens at layer boundaries → cross-layer bugs
- Didn't think about code patterns repeating → duplicated code everywhere
- Didn't think about edge cases → runtime errors
- Didn't think about future maintainers → unreadable code

These guides help you **ask the right questions before coding**.

---

## 项目约定

- CI：默认使用同一套构建、检查、打包和发布流程，正式版和预发布版只根据 Tag 区分；不额外设计流程分支。确需分支时，必须先与用户沟通并获得明确批准。
- 给人看的文档：README、使用说明、更新记录、PR 等须直白易读；若用户读起来费力，或要先琢磨词语才能理解正文，就视为不合格，必须改写。仅供 AI 阅读的文档不受这条表达要求限制，以准确、简洁为准。

### 文档完成与分阶段实现

- 需求覆盖原始目标、约束及用户确认的补充决定，既不遗漏含义，也不擅自增减范围；没有阻塞当前阶段的未决问题时，标记“需求就绪”，不为润色或追求完整感反复扩写。
- 设计和 Spec 只需说明当前阶段的用户操作与预期结果、必要依赖及职责、连接方式和输入输出、关键失败处理与最小检查；足以开始实施时标记“本阶段设计就绪”。职责划分不规定固定层数、crate 或库数量，也不要求预先定义远期接口。
- 以上是停止补写的文档标记，不是程序状态字段或自动化，也不代表功能已实现或获用户批准；实施仍须遵守现有批准要求。
- 每阶段先选一个能实际跑通并观察结果的功能，实现后验证，再规划后续。仅文档齐全或编译通过不算功能完成；未实现的完整需求必须保留，不得静默丢弃。
- 无依赖的功能可以并行；先约定必要的连接方式和各自结果，再逐项验证，不要求每项功能都先规划到最终形态。
- 仅在需求新增或变化、用户决定变化、实际验证出现反证，或缺少当前阶段必要信息时，重新打开受影响部分并更新原文；不重写整套设计或另写重复报告，也不为假想未来或追求优雅提前堆设计。
- 当前阶段已知的安全、数据丢失、对外承诺及难以撤回的决定仍须提前弄清，不能以少设计为由跳过。

## Available Guides

| Guide | Purpose | When to Use |
|-------|---------|-------------|
| [Code Reuse Thinking Guide](./code-reuse-thinking-guide.md) | Identify patterns and reduce duplication | When you notice repeated patterns |
| [Cross-Layer Thinking Guide](./cross-layer-thinking-guide.md) | Think through data flow across layers | Features spanning multiple layers |

---

## Quick Reference: Thinking Triggers

### When to Think About Cross-Layer Issues

- [ ] Feature touches 3+ layers (API, Service, Component, Database)
- [ ] Data format changes between layers
- [ ] Multiple consumers need the same data
- [ ] You're not sure where to put some logic
- [ ] You are adding an event kind, JSONL record, RPC payload, or config field
- [ ] UI / command code starts casting raw payload fields directly

→ Read [Cross-Layer Thinking Guide](./cross-layer-thinking-guide.md)

### When to Think About Code Reuse

- [ ] You're writing similar code to something that exists
- [ ] You see the same pattern repeated 3+ times
- [ ] You're adding a new field to multiple places
- [ ] **You're modifying any constant or config**
- [ ] **You're creating a new utility/helper function** ← Search first!
- [ ] Two files read the same untyped payload field with local casts
- [ ] Multiple branches update the same derived state from `kind` / `action`

→ Read [Code Reuse Thinking Guide](./code-reuse-thinking-guide.md)

### When Verifying AI Cross-Review Results

- [ ] Reviewer claims "user input can be malicious" → Check the actual data source (internal manifest? user config? external API?)
- [ ] Reviewer flags "missing validation" → Is the data from a trusted internal source?
- [ ] Reviewer says "behavior change" → Read the code comments — is it intentional design?
- [ ] Reviewer identifies a "bug" in test → Mentally delete the feature being tested — does the test still pass? If yes → tautological test

**Common AI reviewer false-positive patterns**:
1. **Trust boundary confusion**: Treating internal data (bundled JSON manifests) as untrusted external input
2. **Ignoring design comments**: Flagging intentional behavior documented in code comments as bugs
3. **Variable misreading**: Not tracing a variable to its actual definition (e.g., Map keyed by path vs name)

**Verification rule**: Every CRITICAL/WARNING finding must be verified against the actual code before prioritizing. Budget ~35% false-positive rate for AI reviews.

---

## Pre-Modification Rule (CRITICAL)

> **Before changing ANY value, ALWAYS search first!**

```bash
# Search for the value you're about to change
grep -r "value_to_change" .
```

This single habit prevents most "forgot to update X" bugs.

---

## How to Use This Directory

1. **Before coding**: Skim the relevant thinking guide
2. **During coding**: If something feels repetitive or complex, check the guides
3. **After bugs**: Add new insights to the relevant guide (learn from mistakes)

---

## Contributing

Found a new "didn't think of that" moment? Add it to the relevant guide.

---

**Core Principle**: 30 minutes of thinking saves 3 hours of debugging.
