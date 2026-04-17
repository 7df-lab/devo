# GitHub 通知感知系统 - 设计文档

## 1. 目标

让 Agent 在每次会话（包括长会话）中自动感知 GitHub 动态，无需用户转述，从而：
- Agent 能主动分析上游变化
- Agent 能追踪我们 PR/issue 的状态
- Agent 基于最新上下文给出建议

## 2. 核心原则

### 2.1 权责边界
- **Agent 全自动执行**：本地代码改进、测试、分析、研究
- **Agent 只分析不执行**：回复评论、更新PR、创建issues等社交行为
- **用户做最终决策**：所有涉及人类形象的操作由用户决定

### 2.2 技术决策原则
- 当不确定时，Agent 应主动分析并给出建议，而非询问用户
- 用户负责战略决策，Agent 负责技术判断和执行
- Agent 应该帮用户做事，而不是冒充用户

## 3. 架构设计

### 3.1 整体流程

```
GitHub (upstream + our fork)
    ↓ 事件触发 (定时 cron)
GitHub Actions 工作流
    ↓ 检测变化 + 写入文件
notifications/github-activity.jsonl
    ↓ Agent 启动时 / 长会话中按需读取
Agent 上下文
    ↓ 分析 + 建议
用户 (最终决策)
```

### 3.2 组件说明

#### A) GitHub Actions 工作流 (数据采集层)
- **位置**: `.github/workflows/github-notifications.yml`
- **频率**: 每30分钟运行一次 (可调整)
- **职责**:
  1. 检查 upstream 新提交 (对比上次检查点)
  2. 检查我们 fork 的 PR 状态变化
  3. 检查 issues/discussions 的评论活动
  4. 将变化写入 JSONL 文件

#### B) 通知文件 (持久化层)
- **位置**: `notifications/github-activity.jsonl`
- **格式**: JSON Lines (每行一个事件)
- **保留策略**: 最近30天的事件 (自动清理)

#### C) Agent 感知层 (消费层)
- **时机1**: 会话启动时 → 读取所有未读事件
- **时机2**: 长会话中 → 每个新问题前快速检查是否有新内容
- **行为**: 分析事件 → 主动汇报建议 → 不自行采取社交行动

## 4. 数据结构设计

### 4.1 通知文件格式 (JSONL)

```jsonl
{"timestamp": "2026-04-18T10:30:00Z", "type": "upstream_commit", "repo": "7df-lab/claw-code-rust", "hash": "abc123", "message": "feat: add new feature", "author": "someone"}
{"timestamp": "2026-04-18T11:00:00Z", "type": "pr_comment", "repo": "bigmanBass666/claw-code-rust", "pr_number": 37, "commenter": "maintainer", "comment_preview": "Looks good but..."}
{"timestamp": "2026-04-18T12:15:00Z", "type": "pr_merged", "repo": "7df-lab/claw-code-rust", "pr_number": 38}
{"timestamp": "2026-04-18T13:00:00Z", "type": "issue_created", "repo": "7df-lab/claw-code-rust", "issue_number": 39, "title": "Bug report: ...", "labels": ["bug"]}
```

### 4.2 事件类型定义

| type | 含义 | 重要程度 |
|------|------|----------|
| `upstream_commit` | 上游新提交 | low |
| `pr_comment` | 我们的 PR 被评论 | high |
| `pr_reviewed` | 我们的 PR 被 review | high |
| `pr_merged` | 上游合并了某个 PR | medium |
| `issue_comment` | issue 有新评论 | medium |
| `issue_created` | 新 issue 创建 | medium |
| `release_published` | 新版本发布 | high |

### 4.3 元数据文件 (用于增量读取)

```json
{
  "last_read_timestamp": "2026-04-18T09:00:00Z",
  "last_notification_timestamp": "2026-04-18T13:00:00Z",
  "unread_count": 3,
  "summary": "自上次阅读后: 1条PR评论, 1次合并, 1个新issue"
}
```

位置: `notifications/github-meta.json`

## 5. GitHub Actions 工作流设计

### 5.1 触发条件
```yaml
on:
  schedule:
    - cron: '*/30 * * * *'  # 每30分钟
  workflow_dispatch:          # 手动触发
```

### 5.2 执行逻辑
```
1. 读取 github-meta.json 获取 last_notification_timestamp
2. 使用 gh API 查询:
   - git log --since=<timestamp> (upstream commits)
   - gh pr list --state all --json ... (PR 变化)
   - gh api repos/.../issues/comments?since=... (issue 评论)
3. 过滤出新的变化
4. 追加写入 github-activity.jsonl
5. 更新 github-meta.json 的 last_notification_timestamp
6. 提交推送回仓库 (或使用 artifact)
```

### 5.3 注意事项
- 需要 `GITHUB_TOKEN` 或 Personal Access Token
- API rate limiting: 合理控制请求频率
- 文件存储在仓库中 (`.github/data/notifications/`) 或使用 GitHub Artifact

## 6. Agent 行为规范

### 6.1 启动时行为
```
1. 读取 notifications/github-meta.json
2. 如果 unread_count > 0:
   a. 读取 notifications/github-activity.jsonl (从 last_read_timestamp 之后)
   b. 分析每个事件的含义和影响
   c. 生成摘要报告给用户
   d. 更新 last_read_timestamp
3. 如果 unread_count == 0: 无需操作
```

### 6.2 长会话中的行为
```
每次收到用户新问题前:
1. 快速读取 notifications/github-meta.json
2. 比较 last_notification_timestamp vs last_read_timestamp
3. 如果有新内容:
   a. 读取新增事件
   b. 如果与当前工作相关: 融入回答上下文
   c. 如果重要但无关当前工作: 在回答末尾附加提醒
4. 更新 last_read_timestamp
```

### 6.3 输出格式示例
```
[GitHub 动态提醒]
📌 你的 PR #37 收到新评论 (来自 maintainer):
   "Looks good but can you also handle edge case X?"
   
💡 我的分析:
   - 这是一个合理的 review comment
   - 建议回复方向: 解释我们已考虑 X，或者补充测试
   
⚠️ 这需要你来决定如何回复。
```

## 7. AGENTS.md 更新

需要在 AGENTS.md 中添加以下章节:

### 新增: 社交边界原则
- 明确区分"Agent 可自主执行" vs "需用户确认"的行为
- 强调 Agent 不能冒充用户进行社交互动

### 新增: 技术决策权
- Agent 负责技术方案选择和分析
- 用户负责最终决策批准
- Agent 应主动提出选项而非被动等待指令

### 修改: Startup Protocol
- 步骤增加: 读取 GitHub 通知文件

## 8. 文件清单

| 文件 | 用途 | 创建/修改 |
|------|------|-----------|
| `.github/workflows/github-notifications.yml` | Actions 工作流 | 新建 |
| `notifications/.gitkeep` | 通知目录占位 | 新建 |
| `AGENTS.md` | Agent 行为规范 | 修改 |
| `docs/plans/2026-04-18-github-notification-design.md` | 本文档 | 新建 |

## 9. 实施步骤

### Phase 1: 基础设施 (本次实施)
1. ✅ 设计文档完成
2. 创建 `.github/workflows/github-notifications.yml`
3. 创建 `notifications/` 目录结构
4. 更新 AGENTS.md

### Phase 2: 测试验证
1. 手动触发 Actions 测试
2. 验证文件输出格式正确
3. 验证 Agent 能正确读取和解析

### Phase 3: 迭代优化
1. 根据实际使用调整频率
2. 优化事件过滤逻辑
3. 可能增加更多事件类型

## 10. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| Actions 失败 | 设置失败通知；Agent 可 fallback 到手动 gh 命令 |
| 文件过大 | 30天自动清理；JSONL 格式易于裁剪 |
| API 限流 | 控制请求频率；使用缓存 |
| 敏感信息泄露 | 不记录 token/密钥；只记录公开信息 |

## 11. 成功标准

- [ ] Agent 能在启动时自动感知最近的 GitHub 活动
- [ ] Agent 能在长会话中检测到中途发生的新事件
- [ ] Agent 对社交类事件只分析不行动
- [ ] Agent 对技术类事件能自主处理
- [ ] 用户不需要转述 GitHub 动态给 Agent
