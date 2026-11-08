# UI 规范

视觉参考 Linear：**侧栏 + 标签条 + 主内容卡片** 三段式。目标是密度高、层级少、默认不用学。

## 1. 窗口结构

```text
┌──────────────┬──────────────────────────────────────────────┐
│ 项目切换器    │ 标签条: 概览 │ M1 证据门 │ 文档: PRD │ +     │
│ 搜索 / 新建   ├──────────────────────────────────────────────┤
│ 概览          │ ┌──────────────────────────────────────────┐ │
│ 我的任务      │ │ 标题                        + 新建        │ │
│ 审查   (n)    │ │ 过滤: 全部 进行中 阻塞 完成   ⚙           │ │
│ Agent         │ │ ──────────────────────────────────────── │ │
│               │ │ 表格 / 主从列表 / 空状态                   │ │
│ 空间           │ │                                          │ │
│ 项目 / 视图    │ │                                          │ │
│ 文档 / 知识    │ └──────────────────────────────────────────┘ │
│ 底部用户       │                                              │
└──────────────┴──────────────────────────────────────────────┘
```

- 侧栏固定 240px，使用窗口底色；主区是 inset 圆角卡片，与底色差一级
- macOS 保留原生红绿灯，顶部留可拖拽区域；窗口最小 960×640
- 标签条承载已打开的视图（项目列表、Issue、文档、Agent 会话），关闭最后一个回到概览
- 主区最大宽度不设限，内容区左右 padding 16–20

## 2. 导航映射

| Linear | temli | 说明 |
|---|---|---|
| Inbox | 概览 | 项目状态 + 决策收件箱 + 队列，默认落地 |
| My issues | 我的任务 | 需要我审批或验收的任务 |
| Reviews | 审查 | 待审查 / 已合并的 run，主从布局 |
| Agent | Agent | 会话列表 + 会话详情 |
| Projects | 项目 | 项目列表（表格） |
| Views | 视图 | 保存的过滤，如「本周待验收」 |
| Loops / Import / Invite | — | 不实现 |
| — | 文档 | 仓库 `docs/` 索引 |
| — | 知识 | `.temli/notes` 索引 |

一级项（概览、我的任务、审查、Agent）用 HeroUI Listbox 单选；空间分组用可折叠 section。计数徽标只用 HeroUI Badge/Chip，不自绘。

## 3. 列表、过滤与详情

- 项目/任务列表：HeroUI Table；列：名称、状态、优先级、Agent、验收、进度；行高 40，hover 整行高亮
- 过滤条：HeroUI Tabs（全部 / 进行中 / 阻塞 / 完成）+ Chip 过滤；右侧 Popover 放视图设置
- 详情：≥ 1200px 分栏（列表 380px + 详情）；更窄时用 HeroUI Drawer 从右侧滑出
- 空状态：插画 + 一句说明 + 主按钮；不写大段引导文案

## 4. Agent 界面

- 会话页：消息流 + 底部输入（TextArea + 发送按钮）；空会话展示 3 张示例卡片
- 审批请求：消息流内联卡片（Card + Button），同时进入概览的决策收件箱；超时默认拒绝
- 运行中展示取消按钮、耗时、token 与费用（适配器支持时）
- 全局入口：右下角 Agent 按钮，新开标签

## 5. HeroUI 约束

- 依赖：`@heroui/react`、`@heroui/styles`；React 19 + Tailwind CSS v4
- 入口 CSS 顺序固定：`@import "tailwindcss";` 之后 `@import "@heroui/styles";`
- **不修改 HeroUI 组件内部样式**：不写覆盖内部类名的 CSS，不复制其内部 DOM 结构，不依赖内部选择器
- 允许：公开 props（`variant` / `size` / `color` / `isDisabled` 等）、主题变量；`className` 只用于布局（flex / grid / margin / width / height）
- 需要新外观时：优先组合现有组件，其次用 HeroUI 主题变量；仍不满足再自建组合组件，但不得重造已有控件
- 图标统一 `lucide-react`，尺寸 16，颜色继承文本；不使用 emoji 作为功能性图标
- 暗色跟随系统，不单独做主题切换入口

## 6. 组件选型

| 场景 | 组件 |
|---|---|
| 导航 / 列表选择 | Listbox、Tabs |
| 数据列表 | Table、Pagination |
| 过滤 / 标签 | Tabs、Chip、TagGroup |
| 详情容器 | Drawer、Sheet、Card |
| 输入 | TextArea、TextField、SearchField、Select |
| 状态 | Badge、Spinner、Skeleton、ProgressBar/ProgressCircle |
| 反馈 | Toast、Alert、Tooltip、Modal |
| 头像 / 用户 | Avatar、AvatarGroup |

## 7. 密度、字号与动效

- 基准字号 **13px**（正文与控件），次级 12px，分组标签 11px；行高 1.45
- 间距走 4px 栅格；侧栏 232px；导航行高 32px；标签页 28px；表头 30px；数据行 36px
- 主卡片圆角 8px + 1px 边框 + 极轻阴影；窗口底色比卡片低一级
- 选中态使用中性灰（HeroUI `tertiary`），强调色只用于主操作按钮与状态
- 过渡统一 120ms、`cubic-bezier(0.2, 0, 0, 1)`，只作用于颜色、背景、边框、阴影、透明度；`prefers-reduced-motion` 下关闭
- 表头 12px 次级色；行 hover 使用窗口底色；标签页关闭按钮仅在 hover 时出现
- 图标统一 16px（内联控件 14px），不混用 15/16
- 不新增对 HeroUI 组件内部样式的覆盖；选中/悬停优先用组件自带 variant
