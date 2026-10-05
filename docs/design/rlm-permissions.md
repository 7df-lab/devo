# RLM Kernel 权限系统设计(Design Doc)

状态:草案 v1(2026-09-19)
范围:`crates/kernel` / `crates/server` / `crates/safety` / `crates/sandbox*` / `crates/core`(ipython)/ `rlm-runtime`(Python)/ 桌面端审批 UI
关联:`.cursor/rules/rlm-target-design.mdc`(产品不变量)。历史:原型实验 `demos/rlm-privilege-once` 已删除,其 once 语义被本设计吸收,session 的 respawn 路径被否决(§9)

---

## 0. 决策摘要(TL;DR)

1. **围栏优先(fence-first)**:kernel 进程本身套 OS 沙箱(复用现有 `windows-sandbox` / `linux-sandbox` / Seatbelt)。围栏内(workspace)自由;围栏外 **fail-closed**,由操作系统拒绝,不依赖任何字符串过滤或 Python 级补丁。
2. **越权即询问**:模型代码碰到围栏拒绝后,走 `host_request` → **现有审批管线**(`authorize_tool_request`,approval.rs:54)→ once / session / always。
3. **最大程度复用**:审批管线、8 种 ApprovalScope、grant cache、Windows restricted-token 沙箱、execpolicy 持久化——大部分现成,但 2026-09-19 审计后三处"现成"要打折:代理 `WebsitePolicy` 类型存在但**无任何运行时消费**(策略执行是净新增);execpolicy **无热重载**(只有 persist 后原地重载,runtime.rs:452 是初始加载);TUI 审批 UI 只会二元 confirm 且 scope 硬编码 `once`(once/session/always 选择目前仅桌面端)。**新建的只有四块**:① `KernelSession::spawn` 的沙箱化接线;② Python 侧薄封装(`rlm.read/write/fetch/install`)与错误翻译;③ 两个新 scope(`PathPrefixPersist`、`HostPersist`,补齐 always 的路径/域粒度);④ **动态授权投放**(§9:Windows 每会话 capability SID + ACL 增删、代理策略、句柄传递)与 fence 推导(仅用于 spawn 与崩溃重建)。另:once 的"结果回填命名空间"是**净新增协议消息**(见 §6.1)。
4. **不引入 audit-hook 边界**:CPython audit hook 只作为可选的诊断/遥测层,不作为安全边界(理由见 §6.4)。避免"双真相源"。
5. **Node→Rust**:kernel host 已经 100% 是 Rust(`crates/kernel` + `crates/server`);Node 侧 RLM 代码是刻意 stub 的死代码,应**删除**而非重写。仍留在 Node 的只有 TUI 与桌面 UI,那是独立决策(§15)。

---

## 1. 背景与问题

RLM 执行面已上线:会话 kernel 可用时,turn 切换到 `ExecutionSurface::Rlm`;registry 保留 `ipython` 与受审批的内部 `bash` handler,但模型 schema 与 ToolSearch 隐藏直接 `bash`,模型的代码执行入口仅为 Python。Python `rlm.bash` 仍在 kernel OS 围栏内启动进程,但不能申请 bash tool 的逐命令额外授权(`crates/server/src/runtime/turn_exec/query.rs:244-258`),kernel 由 `crates/kernel/src/session.rs:330` spawn 的 `python -m rlm.repl` 长驻,命名空间经 `session-artifacts/<id>/kernel.dill` 跨重启恢复。

当前效果(effect)通道有三类,权限覆盖极不均匀:

| 通道 | 示例 | 现状 |
|---|---|---|
| A. 宿主中介 | `host_request`:bash / write / edit / mcp.call / web.search / web.fetch | **bash/write/edit/web 完整审批**:Plan 硬拒 → `ToolPermissionRequest` → `bridge.permission.check` → `authorize_tool_request` 级联 → UI → grant cache → 沙箱化执行(`crates/server/src/runtime/kernel_host_bridge.rs:316-405`)。**例外——MCP 是现网缺口**:`mcp.call` 的中介 handler 存在(kernel_host_bridge.rs:407-461)但 Python 侧不调用,`mcp.py:476-480` 走 kernel 内 `_registry` 本地直连,审批管线完全旁路(fence 上线前必须先收口,见 §16-R7) |
| B. kernel 内原生 I/O | `open()` / `os.system` / `subprocess` / `socket` | **零拦截**。rlm-runtime 全包无任何权限代码;唯一的"管控"是 prompt 劝阻 |
| C. ipython 工具本身 | 提交一个 cell | `capability_tags: vec![]`(`ipython.rs:49-50`)→ `ResourceKind::Custom("ipython")` → `static_policy_allow()`(`approval.rs:1017`)→ Interactive 模式下**从不询问** |

且 kernel 进程是裸 `Command::new(python)`(`session.rs:331`),不读 `sandbox_profile`,即:**模型拿到一个不受限的持久解释器**。这与产品不变量"code runs in isolated sandboxes"(`rlm-target-design.mdc` #4)冲突。

---

## 2. 现状盘点:可复用资产与缺口

### 2.1 资产清单(直接复用,带接线点)

| 资产 | 位置 | 在本设计中的角色 |
|---|---|---|
| 审批级联(mode→cache→policy→execpolicy→hook→AutoReview→UI) | `crates/server/src/runtime/approval.rs:54-191` | 唯一决策点,不改结构,新增请求来源 |
| `ToolPermissionRequest` / `ResourceKind` | `crates/core/src/tools/router.rs:865-886`、`crates/safety/src/lib.rs:461` | 表达"Python 读/写某路径、执行某命令"的审批请求,现成 |
| 8 种 `ApprovalScopeValue` | `crates/protocol/src/approval.rs:31`(Once/Turn/Session/PathPrefix/Host/Tool/CommandPrefix/CommandPrefixPersist) | once/session/always 语义基础 |
| scope 落地 + grant cache | `crates/server/src/runtime/session_actor/approval_scope.rs:10-69`、`approval.rs:1509-1559` | **PathPrefix 已把授权目录写进 `RuntimePermissionProfile.readable/writable_roots`**(`approval_scope.rs:72-93`)——这正是 fence 的数据源,fence 重建因此免费 |
| always 持久化机制 | `persist_command_prefix_rule`(`approval.rs:637-656`)→ execpolicy 追加 `~/.devo/rules/default.rules`(`crates/execpolicy/src/amend.rs:66-82`) | 命令 always 已有;路径/域 always 按同模式新增 |
| Windows OS 沙箱 | 专用低权账户 + `CreateRestrictedToken` + capability-SID ACL + WFP(`crates/windows-sandbox/*`);包装样板 `shell_exec/launch.rs:324-388` → `devo.exe --run-as-windows-sandbox --permission-profile <JSON> --`(`wrapper.rs:38-116`) | kernel spawn 套用同一包装;`FileSystemSandboxPolicy` entries(`protocol/permissions.rs:180`)表达 read/write/deny |
| Linux/macOS 沙箱 | bwrap/Landlock+seccomp(`crates/sandbox/src/profiles.rs:243-399`)、Seatbelt(`seatbelt.rs`) | 同上 |
| `HostRequestHandler` 协议 | `crates/kernel/src/session.rs:24`,fail-closed 默认 `deny_host_handler`(:27);`host_reply` 走独立 stdin 锁(:684-689) | 新增权限动作只扩 `kernel_host.rs:29` 的 dispatch 表,协议不动 |
| `HostBridge`(每 turn 能力上下文) | `crates/server/src/kernel_host_bridge.rs:35-59`,已携带 `sandbox_profile`(:45) | fence 与 per-turn 策略的挂点 |
| 审批恢复 | `approval_resume.rs:170`(进程重启后恢复审批并续 turn) | kernel 场景免费获得 |
| UI 审批流 | `ChatPermissionFlow` + `buildApprovalChoices`(`apps/desktop/src/renderer/components/chat/chat-permission-options.ts:69-149`),`permission.asked` 事件带 `availableScopes` | 新增 Python 场景的文案与选项,机制不动 |
| kernel 状态快照 | 每 cell 成功后持久化 `kernel.dill`(`ipython.rs:529`);restore(:432-459) | fence 扩张重启(lazy respawn)的基础 |
| 语义原型(已删除) | ~~`demos/rlm-privilege-once/`~~:once=host 代办+`save_as` 回填被吸收进 §6.1;session=扩 fence+respawn 被否决(§9);对抗用例清单(curl 走私、`$VAR` 路径技巧)移入 §13-1 测试计划 | — |

### 2.2 缺口(需新建)

1. **kernel 进程沙箱化**:`KernelSessionConfig` 无 fence 字段,spawn 不经过任何沙箱路径。
2. **Python 侧薄封装与错误契约**:模型碰到 OS 拒绝(`PermissionError`/`OSError`)时,需要一条被引导的正门(`rlm.read()` 等),以及可自解释的错误信息。
3. **fence 推导与重建**:从 session 的 `RuntimePermissionProfile` + sandbox profile 推导 kernel fence;重启/恢复时幂等重建。
4. **always 的路径/域持久化**:现状只有 `CommandPrefixPersist` 落盘;`PathPrefix`/`Host` 是会话内存级。
5. **`[permission]` 规则的 kernel 类别**:`ToolFilter`(`crates/config/src/permission.rs:26`)无 kernel/ipython 变体;`PermissionAccess`(`crates/safety/src/permission/types.rs:5`)无 Python 变体。
6. **safety 静态分析只懂 bash**:无 Python AST 级文件访问/命令提取(tree-sitter-python 已是依赖,后置项)。

### 2.3 勘误表(2026-09-19 全量核对后修正的引用,正文已按此为准)

- grant cache 的**写入**位置是 `session_actor/approval_scope.rs:10-69`(经 `approval.rs:524-548` 调用);`approval.rs:1509-1559` 是匹配谓词 `permission_cache_matches`。
- `is_banned_prefix_suggestion` 定义于 `crates/core/src/tools/exec_policy_amend.rs:58`(`approval.rs:1213` 是调用点)。
- `FileSystemSandboxPolicy`(:180)与 `ReadDenyMatcher`(:195)在 `crates/windows-sandbox/src/protocol/permissions.rs`,不在 `crates/protocol`。
- `host_reply` 用的是**同一把** stdin 锁;死锁规避靠独立的 `execute_gate`(session.rs:322)+ handler 无锁 await(session.rs:682)。
- `HostRequestHandler` 是 type alias 回调(`Arc<dyn Fn...>`),非 trait。
- 错误信封构造 `error_reply` 在 `kernel_host.rs:324`(:304-322 是信封规范化)。
- `repl.py:143` 是 `_fail_pending_host_requests` 定义,调用点在 shutdown(:1046)/EOF(:1065)。
- 子代理真实路由是 `task/start` → `spawn_agent`(`crates/server/src/runtime/agents.rs:20`),`rlm.run` 只是 stub。
- `kernel_host_bridge.rs` 的中介路径:bash/write/edit 为 :316-405(316-569 还含 mcp/web/question);`plan_mode_denies_mutation` 在 :155-157。
- `.devo` 无 deny ACE,是 profile 只读子路径(windows-sandbox protocol/permissions.rs:401);deny-write ACE 实际用于 `.git/.codex/.agents`。
- execpolicy 无热重载:`runtime.rs:452-454` 是构造时初始加载。
- 沙箱对照审计(codex v0.150.1)的完整结论见 `crates/windows-sandbox/DEVO_PATCHES.md`(Windows 分叉缺口)与本文 §5.2/§9(平台机制修正)。

---

## 3. 威胁模型与设计原则

**威胁模型**:kernel 中的代码是模型生成的,模型可能被 prompt injection 污染——按**半对抗**对待。明确不设防的目标:模型读写已授权目录内的内容、在围栏内自由计算(这本来就是产品意图)。要防的:越出授权面的读、写、命令、网络,以及对防线的绕过(字符串过滤器绕过、native 代码逃逸)。

**原则**:

- **P1 OS 边界是唯一强制层**。原型实验已证明:字符串级命令检查、`socket.socket` 补丁、prompt 劝阻都不可作为边界;只有"路径不在挂载/ACL 里"、"WFP 断网"这类 OS 事实可靠。
- **P2 fail-closed**。沙箱不可用 → 不给 RLM 面(回退 Discrete,README 已定义该回退为 bootstrap 默认),而不是降级为裸 kernel。
- **P3 单一 choke point**。所有效果通道(中介工具、kernel 原生 I/O 的升级请求、automation)汇入同一个 `authorize_tool_request` 与同一个规则库。
- **P4 fence 必须可重建**:fence = `derive(session 权限状态)`,纯函数,不依赖 kernel 进程内任何状态(kernel 内不可信)。
- **P5 子代理只紧不松**:`rlm_prompts.rs:334` 已把"权限继承永不比父宽松"写进 prompt;围栏模型把它变成机制。
- **P6 围栏内零打断;打断时在授权那一刻完成泛化**(UI 已是这个形态)。

---

## 4. 总体架构

```
┌─ 模型(root execution tool: ipython; bash schema hidden) ───┐
│  cell = 任意 Python                                             │
│   ├─ 围栏内:open()/rlm.bash()/计算 …… 直接执行,零审批           │
│   └─ 碰到围栏拒绝 ──► rlm.read/write/fetch/install(正门)      │
└──────────┬───────────────────────────────────────────────────┘
           │ host_request: fs.read / fs.write / bash.preflight /
           │             install / permissions.request …
┌──────────▼───────────────────────────────────────────────────┐
│ Rust host(crates/kernel session.rs + crates/server)           │
│  dispatch(kernel_host.rs) → 权限级联(approval.rs)              │
│   ├─ 命中规则/cache → 放行,host 代执行,结果回填(save_as)      │
│   └─ 需询问 → approval/command|fileChange|permission/request   │
│        → UI once/session/always → scope 落地(approval_scope.rs)│
│  fence = derive(RuntimePermissionProfile + sandbox_profile)     │
└──────────┬───────────────────────────────────────────────────┘
           │ spawn 时套沙箱(Windows restricted token /
           │ Linux bwrap / macOS seatbelt)
┌──────────▼───────────────────────────────────────────────────┐
│ python -m rlm.repl(OS 围栏内:workspace rw,网络默认断,deny 只读)│
└──────────────────────────────────────────────────────────────┘
```

四层职责:

- **L0 进程围栏(OS)**:强制边界,fail-closed,不含任何"询问"逻辑。
- **L1 正门 + 错误翻译(Python 薄层)**:引导与体验,不是边界。try-direct → 失败转 `host_request`。
- **L2 审批与授权(Rust host)**:唯一决策点;once=代办+句柄、session=cache+动态凭证投放(§9)、always=持久化规则。
- **L3 策略与持久化**:`config.toml [permission]` / `sandbox.toml` / `~/.devo/rules/*.rules` / rollout(approval items + fence events)。

---

## 5. Kernel 沙箱化(L0)

### 5.1 fence 数据结构

不发明新结构,以现有 `SandboxProfile`(`crates/sandbox/src/profiles.rs:27`)为核:

```rust
// crates/kernel —— 新增(示意)
pub struct KernelFence {
    pub profile: devo_sandbox::SandboxProfile, // read_only / read_write / deny / restrict_network
    // RLM 特化:
    pub python_ro: Vec<PathBuf>,   // 解释器与 rlm-runtime 只读根(必须可读)
    pub site_packages_rw: Option<PathBuf>, // 会话 venv(见 §10)
}
```

推导(纯函数,P4;用于 spawn 初始化与崩溃后的重建,授权期间的动态变化走 §9 的投放机制,不经此函数):

```
fence(session) = SandboxProfileFor(session.preset)
                 + roots(session.permission_profile)   // PathPrefix 授权已写入(§2.1)
                 + [.devo 元数据 deny 只读]            // windows-sandbox 默认保护,复用
                 + per-session cap SID(Windows)/继承 dirfd 通道(POSIX)(§9;初始零授权)
```

### 5.2 接线点

`KernelSessionConfig` 增加 `fence: Option<KernelFence>`;`KernelSession::spawn`(`session.rs:330`)在构造 `Command` 后,若 `fence` 存在则调用各平台包装器——**复用 `shell_exec/launch.rs:324` 的既有样板**:Windows 走 `devo.exe --run-as-windows-sandbox --permission-profile <JSON> python -m rlm.repl --`;Linux 走 bwrap argv(`crates/sandbox/src/profiles.rs` / `crates/linux-sandbox` 已有);macOS 走 Seatbelt。`ensure_kernel`(`ipython.rs:401`)的调用方(query.rs:196)从 turn 上下文把 fence 传入——`HostBridge` 已携带 `sandbox_profile`(`kernel_host_bridge.rs:45`),补一根线即可。

网络(三平台同构,细节见 §9 矩阵):kernel 的网络出口被 OS 收敛到唯一通道——Windows = Firewall 账户级规则仅放行 localhost 代理端口(全量断网其实是 per-account 防火墙规则,WFP 只封 ICMP/DNS/DoT/SMB 端口,审计修正 2026-09-19);Linux = bwrap `--unshare-net` + spawn 时附带的预连接代理 socket;macOS = Seatbelt 仅放行 localhost 代理(或预连接 socket)。出网与否、放行哪些域,目标是由代理策略决定(默认 deny,等价断网)——**但现状校验**:`WebsitePolicy`(`crates/sandbox/src/network_policy.rs:188`)自述未被任何运行时消费,代理 crate 现为无策略裸中继(348 行,绑 `127.0.0.1:0` 随机端口),而 Windows 防火墙回环放行表是 setup 时静态端口——**随机端口与静态放行表失配是现行 bug**(已列 re-vendor 修复清单,`crates/windows-sandbox/DEVO_PATCHES.md`);每请求策略执行是 P2 净新增,codex 参考实现在 `network-proxy/src/http_proxy.rs`(CONNECT/明文/SOCKS5 三处 evaluate)。网络授权 = 代理策略编辑,永不需要改进程属性重启。

### 5.3 沙箱不可用时:显式降级,绝不静默

原则从"没有围栏就没有 RLM"调整为"**没有围栏用户必须知情**"(2026-09-19 定稿):

- kernel spawn 套不上沙箱时,RLM 面**仍然可用**,但降级必须**响亮且可记忆**:
  - TUI:一次性醒目警告 + 状态栏持续指示(如 `UNFENCED` 角标);
  - Desktop:会话横幅;
  - 警告文案明说失去的保护("代码将以你的完整用户权限运行");
  - 用户选择可记忆:每次询问 / 本次允许 / 不再提醒;
  - rollout 记 `fence-off` 事件(审计 + 恢复)。
- Windows 沙箱需要一次性提权 setup(`windows-sandbox/src/setup.rs`);未 setup = 降级态 + 引导完成 setup 的入口,而不是拒绝服务。
- 显式 opt-out:`[permission] sandbox_profile = "off"`(preset FullAccess 已映射 "off",`safety/src/lib.rs:151`)同属降级态,同样警告 + 记事件。
- 底线不变的部分:有 deny-read 的 profile 禁止脱沙箱(`approval.rs:568-586` `check_escalation_unsandboxed_forbidden`)——deny-read 是唯一防线时,静默去掉围栏等于欺骗,此处维持硬拒。

### 5.4 模式联动(去重启化)

- **Plan 模式**:进入 = 回收写授权(Windows 删写 ACE 即时;POSIX 门面关闭写 dirfd)且中介路径硬拒兜底;退出 = 重新投放。双向动态、不重启(§9);已在写的句柄需协作关闭。这样 Plan 的硬拒(`plan_mode_denies_mutation`,`kernel_host_bridge.rs:155-164`)从"只覆盖中介动作"升级为"覆盖 kernel 全部效果通道"。
- **PermissionMode**:Yolo → 投放全部授权 + 代理全放;Deny → 回收全部写授权。`permission_mode_authorization`(`approval.rs:1622`)语义自然延伸到投放层。

---

## 6. 执行面:正门 API 与错误契约(L1)

### 6.1 核心模式:try-direct → ask

```python
# rlm/(新增 fs 门面;伪代码)
async def read(path, *, save_as=None):
    try:
        return open(path, encoding="utf-8").read()          # 围栏内直接成功
    except OSError as e:                                    # 不是只捕 PermissionError!
        r = await host_request("fs.read", {"path": path, "errno": e.errno})
        # host 裁决:真不存在(回 ENOENT 语义) vs 被围栏挡住(进审批)
        if r["status"] != "ok":
            raise PermissionError(_deny_message(path, e, r)) # 见 6.2
        return r["result"]["content"]
```

**为什么捕整个 `OSError` 家族**:Linux 围栏对外的表现常常是 ENOENT(路径被挂载隐藏),不是 EACCES——只捕 `PermissionError` 会让正门在 Linux 上永远不触发。kernel 内无法区分"不存在"与"被挡",所以交给 host 裁决(host 知道 fence,能 canonical 路径)。

`write` / `fetch` / `install` 同型。要点:

- **once = 短命凭证,kernel 原生执行**(2026-09-19 定稿,取代"host 代执行+回填"的旧表述):授权后 host 向活 kernel 投放一次性凭证——Windows 为单文件 `DuplicateHandle` / 短 ACE 窗口,Linux/macOS 为经 `SCM_RIGHTS` 传入的已打开 fd——**kernel 在自己的进程上下文里完成这次读写**(env/cwd/句柄/状态全保真,大文件、流式、seek 都在 kernel 侧),操作完成(或超时)host 立即回收凭证。同一套投放机制,三种寿命(once=即收,session=常驻,always=落盘)。诚实代价:投放与回收之间有小窗口,模型理论上能碰第二次——这是意图粒度问题不是安全边界(边界仍是 fence);不可接受的场合退回 host 代办。
- **无法凭证化的操作退化为上下文保真代办**:需要出围栏执行的命令(kernel 无法在自己上下文里越 fence exec)由 host 执行,但 kernel 随请求带上 `os.environ` + `os.getcwd()` + stdio,host 原样复刻环境,回传退出码与输出——子进程能感知的一切就是 env/cwd/stdio,保真后上下文损失近零。结果回填命名空间(save_as)是**净新增协议消息**(repl.py 现只有快照恢复与 `_`),排期时按新协议能力算。
- **session**:授权进 grant cache(对后续中介请求立即生效);并按 §9 投放持久凭证——kernel 全程不重启。
- 读写大文件:宿主代办路径的预算要为 RLM 调大(kernel_host_bridge 现为 `output_limit_bytes=32KiB`、wall 6s,`router.rs:855-856`;RLM 场景建议 fs.read 放宽到 1MiB / 30s,bash 已有 `bash.start` 异步路径)。

### 6.2 错误契约(模型自解释)

拒绝时抛出的 `PermissionError` 文本必须让模型能自愈,包含:被拒路径/命令、允许范围(`当前可写:C:\proj;可读:C:\proj, C:\libs`)、正确姿势(`用 rlm.write() 或先请求授权`)。`{status:"error"}` 信封(`kernel_host.rs:304`)→ Python 侧 `_parse_host_reply`(`rlm/__init__.py:89`)已经把 error 变异常,文案在 Rust 侧生成。

同时把 OS 拒绝翻译成引导:rlm-runtime 启动时安装一个**仅做错误翻译**的 `sys.excepthook` 级提示不可行(太粗),改为:正门 API 文档 + root prompt 增加一条("碰到底层 PermissionError/EACCES(而非 rlm.* 抛出的)时,改用 rlm.read/write 或 rlm.request_bash")。这是引导,不是边界(P1)。

### 6.3 bash 的双路径语义

| 路径 | 执行位置 | 审批 | 适用 |
|---|---|---|---|
| `bash()`(kernel 内,`rlm/bash.py`) | kernel 围栏内(继承 restricted token) | **不逐条审批**;例外见下 | workspace 内命令(构建、测试、git)——RLM 主路径,**围栏即授权** |
| `host_request("bash")`(已有) | host 沙箱/unsandboxed | 完整审批级联(现状) | 围栏外/需 escalation/需网络的命令 |

理由:RLM 工作流单 turn 内大量短命令,逐条审批会摧毁该执行面的价值;围栏已把损害半径限制在 workspace。**例外——Forbidden 硬规则必须对围栏内 bash 也生效**(如 `rm -rf $HOME`、对 deny 根的写):`bash()` 在 spawn 前发一条轻量 `bash.preflight`(发 argv,回 allow/deny,命中 Forbidden/危险启发才 deny,**不弹 UI**)。IPC 一次毫秒级,可接受;策略只在 host,满足 P3/P4。风险与备选见 §16-R2;更强的参考架构是 codex 的 execve 级拦截(打补丁的 zsh + escalation server + fd 转发,§9 矩阵 macOS 格注),P2 时评估是否值得引入。

### 6.4 audit hook 的定位:不做边界

`sys.addaudithook` 能拦 `open`/`subprocess`/`socket`,但:① 防不住 `ctypes`/native 扩展绕过(封 import 又引向军备竞赛);② 与 OS fence 形成**双真相源**,两者漂移时产生无法解释的行为;③ 本设计里 OS 已经 fail-closed,hook 无增量强制力。结论:v1 不做;若后续想要遥测(统计模型绕过正门的频率)、或想要"沙箱 off 时的 best-effort 警示",再以纯观测目的引入。

---

## 7. host_request 协议扩展(L2 入口)

全部新动作进 `kernel_host.rs:29` 的 dispatch 表,未注册动作走现有 unknown error(fail-closed);遵守既有死锁规则(host_reply 独立 stdin 锁;`interrupt` 不等 execute_gate;shutdown/EOF fail pending requests,`repl.py:143`)。

| action | 语义 | 权限请求 |
|---|---|---|
| `fs.read` / `fs.write` | 单文件读/写(once=代办+回填;cache 命中=代办不再问) | `ResourceKind::FileRead/FileWrite` + path |
| `fs.stat` / `fs.list` | 只读元数据(可先直接尝试,失败再问) | FileRead |
| `bash.preflight` | argv → allow/deny,不弹 UI | Forbidden/危险启发,拒绝即回 error |
| `install` | 宿主侧 `uv pip install` 进会话 venv(§10) | `Custom("rlm.install")` 或新 ResourceKind;once/session |
| `permissions.request` | **preflight 批量**:声明意图("写 src/ 下 5 文件 + 跑 npm test")→ 一次人审 | 合成一组 ToolPermissionRequest,UI 聚合 |
| `fence.status` | 只读:当前 fence 摘要(供模型与调试) | 无 |

`permissions.request` 是打断次数的关键杠杆:模型通常提前知道意图,一次批审覆盖一批操作(等价于 acceptEdits / plan-approve 在 kernel 世界的对应物)。落地为"多个请求的一次聚合 UI",复用现有 approval item 持久化。

---

## 8. 授权语义(once / session / always)

映射到现有 8 个 scope,不新增枚举除下述两个:

| 用户选择 | 现有 scope | kernel 场景落地 |
|---|---|---|
| 本次 | `Once` | **短命凭证投放**:单文件句柄 / 短 ACE 窗口,kernel 原生执行后即回收;不写 cache(approval_scope.rs 现状即此语义) |
| 本 turn | `Turn` | 现状按工具粒度;kernel 场景建议细化为"本 turn 内该 (action, resource) 模式" |
| 本会话 | `Session` / `PathPrefix` / `CommandPrefix` / `Host` | **立即**写 grant cache(中介路径即时免问);PathPrefix 同时已写入 `RuntimePermissionProfile` roots → fence 重建时自动带上(§9) |
| 总是 | `CommandPrefixPersist`(已有) | execpolicy 追加规则,热重载(runtime.rs:453)——kernel 场景免费复用 |
| 总是(新) | **`PathPrefixPersist`** | 新增:把路径前缀规则持久化。落点二选一:(a) `config.toml [permission] rules`(tool=`Read`/`Edit` + glob,`crates/config/src/permission.rs:26` 已支持此形状,只差 ToolFilter 扩展);(b) sandbox profile 追加 read_write 根。推荐 (a):可版本化、可放进项目 `.devo/config.toml` 与用户级,语义与 `PermissionRule` 一致 |
| 总是(新) | **`HostPersist`** | 新增:append `network_rule(host=..., decision="allow")` 到 `~/.devo/rules/default.rules`(execpolicy 的 amend 机制换一种规则类型,同文件锁与幂等模式) |

**泛化发生在授权那一刻**(P6):Python 场景的 UI 选项(在 `buildApprovalChoices` 上加文案分支):仅此文件 / 此目录 / 此子树(读|写);仅此命令 / 带参前缀(既有);仅此域。宽前缀防护复用 `is_banned_prefix_suggestion`(`approval.rs:1213`)思路,并加一条:解释器类 argv[0](bash/python/pwsh/node/cmd)不允许 pattern 泛化,只能 exact 或显式全放行。

**once 的计数在 host**:once=一次效果(一次代执行),不是一次 kernel 生命周期;由审批管线现状语义保证(不写 cache)。

**replay**:rollout 已持久化 approval item;新增 fence 事件(spawn/widen/opt-out)进 `InternalRecord`,使 replay 与崩溃恢复后 fence 可审计。进程重启恢复走 `approval_resume.rs:170` 既有路径。

---

## 9. 授权投放(核心机制):永不重启 kernel

**硬约束:除崩溃恢复外,授权流程不得重启 kernel。**理由:kernel 是不可复现的状态容器——`kernel.dill` 只能恢复 picklable 的命名空间绑定,而 RNG 状态(`random`/`numpy`/`torch`,在 C 层模块状态里,不在 `ns` 字典里)、已打开句柄、线程、C 扩展内部状态、bash 子进程树全部会丢;restore 后重跑 cell 也无法恢复确定性。任何"扩 fence = snapshot→respawn→restore"的方案(含 demo 的 session 路径)都与此冲突,**予以否决**。

授权的语义因此是:**向一个活着的进程投放资源侧凭证(delivery),进程属性永不重建**。

| 能力 | Windows | Linux | macOS |
|---|---|---|---|
| 目录树读/写 | **每会话 capability SID + 动态 ACE**:spawn 时 token 预置本会话 read/write-cap SID(零 ACE → 零权限);授权 = host 在新根 DACL 加该 SID 的 allow ACE,即时生效、可撤销(删 ACE)。**裸 `open()` 直接可用**。`windows-sandbox/cap.rs` 已能创建 capability SID,只需从"每根一个 SID"(`workspace_write_cap_sid_for_root`)改为"每会话一个共享 SID"——动态投放 + 并发会话隔离一起获得 | **目录句柄传递**:host 打开授予根(`O_DIRECTORY`)→ 经 spawn 时附带的 Unix socketpair 以 `SCM_RIGHTS` 传入 dirfd → rlm 门面用 `os.open(path, dir_fd=fd)` 相对解析(读/写/新建/`renameat` 全支持)。经门面原生可用;裸路径解析仍被 bwrap mount 挡住(fence 不变);不可撤销(协作关闭)。**两个 2026-09-19 审计修正必须落实**:① dirfd 必须在"以授予根为 bind-mount 顶点"的私有挂载上下文中打开(host 经 `unshare(CLONE_NEWUSER|CLONE_NEWNS)`+bind mount,或 `open_tree(OPEN_TREE_CLONE)`)——直接传 host 主挂载树上打开的 fd,`openat(fd,"../..")` 会沿 host 挂载树爬到根,而 bwrap `--unshare-user` 不改 uid,逃逸即用户全权限,strict 围栏形同虚设;bind 顶点才钳制 `..`。② 传入的 fd 按 codex `fd_mount.rs` 的模式做真伪校验((dev,ino) 与授予目标一致才接受),防调包。后期升级 FUSE `/grants` 挂载点 → 裸路径可用、host 动态映射 | **目录级动态投放 v1 不可行(2026-09-19 审计结论)**:Seatbelt profile 在 spawn 时固定,会话中途新授予的目录经 dirfd `openat` 解析出的路径不在 allow 列表,被 Seatbelt 自己拒绝——"同 Linux"不成立。macOS v1 = 单文件句柄(host 预打开经 `SCM_RIGHTS` 传 fd,`os.open` 的 `dir_fd` 仅对 spawn 时已授权目录有效)+ 宿主代办兜底;目录授权只出现在 fence 重建(spawn)时。不采用 Seatbelt extension SPI(私有 API,跨版本脆弱)。codex 同样确认活进程不可加宽,其解法是 execve 级拦截 + 沙箱外 fork(可作 §6.3 的参考架构) |
| 单文件/细粒度 | host 打开 → `DuplicateHandle` 注入 kernel 进程 → 句柄值经现有 JSON 协议回传 → `msvcrt.open_osfhandle` 包装成普通 file 对象 | `SCM_RIGHTS` 传 fd | `SCM_RIGHTS` 传 fd |
| 网络强制收敛到代理 | WFP:仅放行 localhost 代理端口,直连全断(账户级) | bwrap `--unshare-net`(仅 loopback);kernel 的网络出口 = spawn 时附带的**预连接代理 socket**(host 建好连接再传入,raw `socket()` 全失败) | Seatbelt profile:`network-outbound` 仅允许 localhost 代理(或同样用预连接 socket,则无需任何网络放行) |
| 网络授权 | **代理 `WebsitePolicy` 编辑**——三平台同构:断网/放行是策略状态,不是进程属性 | 同左 | 同左 |
| 兜底(全平台) | **宿主代办 + 免重复询问**:kernel 里的代码不直接碰围栏外的文件,而是调 `rlm.read("/outside/x")`,这条请求经现有 `host_request` 通道发给 Rust 宿主,宿主用自己的完整权限读文件、把内容作为返回值发回 kernel(kernel 不重启、围栏不变);首次问人,之后同类请求命中授权缓存不再弹窗。每操作一次本地 IPC(亚毫秒)。它不依赖任何 OS 特性,三平台同一份实现,是 P2 投放机制做成前的唯一路径,也是投放机制覆盖不到的场合的保底 | 同左 | 同左 |

**句柄传递通道的 spawn 布线**(三平台统一):host 在 spawn 前创建 `socketpair(AF_UNIX)`,一端留给 host,另一端作为继承 fd 传给 kernel——**bwrap 默认关闭继承 fd,argv 必须显式加 `--pass-fd`(现仓库未用该旗标,审计修正 2026-09-19;codex 的 bundled bwrap 还提供 `--ro-bind-fd`,spawn 时授权可直接用它挂载)**;Seatbelt 保留继承 fd,无需处理。不依赖任何网络命名空间,与 fence 正交。单文件句柄走 Windows `DuplicateHandle`(host 对子进程持 `PROCESS_DUP_HANDLE`),POSIX 走 `SCM_RIGHTS`,句柄值都经现有 stdin/stdout JSON 协议回传。

**撤销语义的不对称恰好匹配 scope**(三平台一致):session = 可收回(Windows 删 ACE 即时;POSIX 门面协作关闭 dirfd + FUSE 模式下 host 撤销映射);once = 一次性句柄,给出即不回收。

**各平台的诚实边界**:
- Windows:读写投递后**裸 `open()` 可用**、可即时撤销——体验最完整。
- Linux:投递后经 rlm 门面原生可用(`rlm.read/write` 直接落到 dirfd,无逐操作 IPC),但裸路径 `open("/outside")` 仍失败,直到 FUSE 升级路径;撤销靠门面协作。macOS:**目录投递 v1 无此列**(见矩阵),单文件句柄 + mediated 是全部。
- Linux/macOS 的 **kernel 内任意直连网络**(不经代理的 raw TCP/UDP)v1 不可投递(Seatbelt profile 与 netns 路由均 fixed at spawn)——统一走 `rlm.fetch`/`rlm.install`;确需直连再评估 Linux veth(netns 内唯一路由指向代理网关)与 macOS 放宽 profile + 重spawn 的取舍,那是唯一可能重新引入 respawn 的场景,需单独审批产品口径。

**once 与上下文(短命凭证,原生执行)**:once 从不另起 kernel,也**默认不再委托 host 重演**——host 投放一次性句柄(§6.1),kernel 在自己的 env/cwd/状态里完成操作,完成即回收。只有无法凭证化的操作(出围栏 exec)才由 host 代办,且必须上下文保真:kernel 随请求附 `os.environ`+`os.getcwd()`+stdio,host 原样复刻,回传退出码与输出(旧表述"pickle 成字节回填/materialize-to-workspace"降级为兜底路径)。

**副产品:模式切换去重启化**。Plan ⇄ 正常 = 加/删 workspace 的写 ACE(§5.4),双向动态;已在写的句柄需协作关闭(记入错误契约与文档)。

**fence 重建(§5.1)仍然保留**,但只服务崩溃恢复与会话冷启动——它不再是授权路径的一部分。session 授权期间若 kernel 意外崩溃,重启后 ensure_kernel 从 `RuntimePermissionProfile`(PathPrefix 已写入 roots)重新推导 fence + host 侧补投 ACE,自动收敛。

---

## 10. 网络与包安装

- 默认:代理策略 **deny-all**(kernel 恒定 spawn 在代理型账户下,§5.2——断网是策略状态,不是进程属性)。web.search/web.fetch 已是中介动作(Network 审批),维持。
- **`rlm.install(pkgs)`**(新动作,§7):产品不变量 #4 要求 pip/uv 可用且被宣传。实现:会话 venv 放在 `workspace/.venv`(写权限天然被 fence 覆盖);host 侧执行 `uv pip install --python <venv> ...`:
  - 网络:install 命令在 host 以注册表域白名单(pypi/files.pythonhosted 等,`WebsitePolicy`)执行,不打开 kernel 网络;
  - 审批:首次"安装 Python 包"询问(once/session);session 内后续 install 免问;
  - kernel 侧只读该 venv(`python_ro` + site-packages 可读)。
- 若模型确需 kernel 内访问网络(罕见):`host_request` 显式请求 → session 授权 → **代理策略放行对应域**(§9,kernel 不重启)。注意:经代理的 HTTP(S) 三平台都可投递;**不经代理的 raw 直连**(raw TCP/UDP)在 Linux/macOS v1 不可投递(fence fixed at spawn),Windows 经 WFP 策略仍可按端口放行——若产品确需 POSIX raw 直连,唯一路径是重 spawn,须单独立项评审(§9 诚实边界)。

---

## 11. 多 agent 与继承

- 子代理 = server 内新增 session actor(真实路径已是 `agent/spawn`,非独立进程),**各自 kernel**。
- 子 kernel fence = `derive(child_profile ∩ parent_fence)`;父会话 grant cache 对子生效(`approval.rs:474-481` 现状)。子代理的授权询问路由到父会话 UI(`permission_host_session_id`,`approval.rs:618-635` 现状)。
- 由此,"Permission, sandbox, and MCP inheritance never relax relative to the parent"(`rlm_prompts.rs:334`)从 prompt 承诺变为机制事实:围栏求交 + 父 cache 只增不减约束。

---

## 12. 桌面端 UI(UX)

- 复用 `ChatPermissionFlow` 与 `permission.asked` 事件(已带 `availableScopes/commandPattern/path/host`);新增:Python 场景文案("代理请求在 Python 内核中**读取** `C:\other\secret.env`")、`PathPrefixPersist`/`HostPersist` 选项、`permissions.request` 的聚合批审视图。
- 打断治理:相同 (action, resource) 的待决询问去重合并;dismiss = deny;Ask 期间 kernel 阻塞但 `interrupt` 仍可达(现有死锁规则保证)。
- 决策日志:approval items 已进 rollout;新增 fence/delivery 事件流(spawn profile / 投放与回收 / preflight deny),既审计也可做活动视图。

---

## 13. 测试计划

1. **对抗测试套件**(新建为 `crates/kernel` / `crates/server` 集成测试;用例源自原 privilege-once 原型的 adversary 清单,文件已随 demo 删除,以此清单为准):curl 走私、`$VAR` 路径技巧、junction/8.3/symlink(Windows 特有:双拼写 canonical,复用 `ReadDenyMatcher` 思路 `protocol/permissions.rs:196`)、subprocess 绕过正门、TOCTOU(fence 判定与实际 open 之间的符号链接替换——代办路径以"打开后按 fd 校验真实路径"收口)。
2. **fence 单测**:每平台 spawn 后探针脚本(尝试 workspace 外读/写/网络)断言全部失败且错误可翻译。
3. **审批语义 golden**:once 不写 cache;PathPrefix 进 roots 并反映在下次 fence 重建;`PathPrefixPersist` 落盘后再新会话首 cell 即免问;解释器 pattern 泛化被拒。
4. **死锁回归**:Ask 挂起时 interrupt 可达;shutdown fail pending。
5. **replay/恢复**:带 fence 事件的会话回放一致;崩溃后 `approval_resume` 续跑。
6. **TUI/桌面 E2E**:现有 tmux/psmux 场景加一条"围栏外写 → 询问 → once → 结果回填"。
7. **投放机制专项(2026-09-19 审计新增)**:dirfd `..` 逃逸(bind 顶点钳制断言)与 fd 调包((dev,ino) 校验);无 ACL 卷(FAT/exFAT/网络卷)自动降级 mediated;授予根的父目录 traverse 链校验(他人 profile 目录);代理端口与防火墙放行表一致性(静态/漂移触发重装);撤销后已开句柄行为(两平台:新 open 被拒、旧句柄继续——断言为已知残余并记录);kernel 进程环境无凭据(断言 `OPENAI_API_KEY` 等 credential 形状变量不出现);并发会话 DACL 写竞争。

---

## 14. 分阶段落地

| 阶段 | 内容 | 验收 |
|---|---|---|
| **P0 fence** | `KernelFence` + 三平台 spawn 接线(Linux 走 strict 档,tmpfs+显式挂载,不是 default_read 的 `--ro-bind / /`)+ 显式降级警告(§5.3)+ kernel env 凭据清理(已完成)+ rollout fence 事件 | 围栏外读/写/网在 OS 层失败;无沙箱时 TUI/Desktop 显式警告、用户选择可记忆、绝不静默 |
| **P1 正门+once** | `fs.read/write/stat/list` + 错误契约 + UI 文案;once=代办+回填 | 模型碰壁 → 一次询问 → 结果可用;对抗套件核心用例过 |
| **P2 session** | cache 联动(已有) + Python 场景 scope 泛化选项 + **动态投放**(Windows 完整:共享 cap-SID + ACE 增删 + 投放 journal/孤儿清扫;POSIX:dirfd 通道 + bwrap `--pass-fd` + bind 顶点打开 + (dev,ino) 校验;macOS:维持 mediated,见 §9)+ 代理每请求策略执行(净新增)+ `bash.preflight` | session 授权后同类操作免问且 kernel 不重启;Windows 裸 `open()` 可用、POSIX `rlm.*` 原生落盘;Forbidden 对围栏内 bash 生效;撤回授权后新开句柄即被拒 |
| **P3 always** | `PathPrefixPersist`(config 规则)+ `HostPersist`(network_rule)+ ToolFilter/PermissionAccess 扩展 | 重启会话后免问;规则文件可读可版本化 |
| **P4 网络/安装** | `rlm.install` + venv 布局 + 注册表域白名单 | 离线 kernel 内 pip 流程一次授权完成 |
| **P5 体验** | `permissions.request` 批审 + 询问去重;单文件句柄传递(三平台 once 的原生体验);(可选)Linux FUSE `/grants` | 单 turn 打断次数 ≤2 的场景测试 |

P0/P1 是安全修复性质,建议先行;P2-P5 可与功能开发交错。

---

## 15. Node → Rust:事实澄清与建议

**前提澄清**(调研结论,证据):kernel host 与 RLM 运行时**已经不在 Node**。生产链路 = Rust server spawn `python -m rlm.repl`(`crates/kernel/src/session.rs:330`),host_request 分发在 Rust(`kernel_host.rs:29`);Node 侧 `apps/tui/lib/coding-agent/src/core/kernel/` 是从 pi-coding-agent 移植后**刻意 stub 化的遗骸**——`repl-manager.ts:5-18` 构造即 throw,由政策脚本 `apps/tui/scripts/complete-stubs.mjs` 维护("Node ReplKernelManager must fail closed — ipython is Rust Native")。Python runtime 本体(`crates/kernel/rlm-runtime/src/rlm/`,约 5000 行)本来就是 Python,也谈不上用 Rust 重写。

因此问题不是"要不要把 RLM runtime 从 Node 迁到 Rust"——**迁移已经完成**——而是两件事:

1. **删除 Node 死代码**(**已完成**,2026-09-19):
   - 已删:`core/kernel/` 的 `repl-manager.ts` / `bootstrap.ts` / `boot-gate.ts` / `state-snapshot.ts`(+test)、`core/rlm-runtime.ts`;`core/kernel/shared.ts` 收缩为纯类型面(UI 仍 import 的 `KernelSentAgentMessage`/`ExecuteResult`/`HostRequestHandler(s)`/`KernelDiffDisplay`/`KernelAttachment` + `isRecord`/`errorMessage`/`createDeferred`);`core/tools/ipython.ts` 收缩为 schema+类型+fail-closed execute(仅静态渲染用);`complete-stubs.mjs` 移除 kernel 相关生成块并改为**默认跳过已存在文件**(`--force` 才覆写)。
   - 事故记录:清理中误跑 `complete-stubs.mjs`,其无条件覆写把 18 个已适配文件(含 Devo 定制的 `cron-jobs.ts`)打回旧 stub;已按幸存代码反推恢复缺失面(`DEFAULT_HEARTBEAT_DELIVERY_MODE`、`HeartbeatCommandResult`、`AgentCronJob` 时间戳字段),并把脚本改为非破坏性防止复发。上游参考 clone 在 `C:\Users\lenovo\Desktop\prime-agent`。
2. **TUI 是否 Rust 化**(大、独立、不建议与权限工作耦合):
   - 现状:产品 TUI = Rust `devo` CLI spawn Node(tsx)跑 `apps/tui`(InteractiveMode 约 1 万行 + `native-agent-connection.ts` 3452 行);协议已统一 Native,UI 是纯前端,与 kernel/权限无关。
   - Rust 化收益:单二进制分发、去掉 Node 进程(~100MB 常驻与启动延迟)、与 server 同语言同类型源(现已有 generate-native-ts,收益有限)。
   - Rust 化成本:TUI 全量重写(ratatui 栈)、UI 迭代速度下降、vendored 生态(该 TUI 源自 pi-coding-agent)后续同步能力丢失。
   - 建议:**解耦决策**。权限系统只依赖 Rust server 与协议,无论 TUI 用什么写都不受影响;TUI Rust 化另立项评估。桌面端(Electron)本来就是 UI 壳,不在讨论范围。

---

## 16. 风险与开放问题

- **R1 Windows 一次性提权 setup**:未 setup 用户直接失去 RLM 面(fail-closed)。需要产品确认:首次进入 RLM 会话时的 setup 引导 UX;以及企业环境(无提权)的降级口径。
- **R2 `bash.preflight` 的定位**:每条围栏内命令一次 IPC;若未来发现延迟/可靠性问题,备选 = 把 Forbidden 规则的只读快照随 fence 下发到 kernel(接受"规则快照可能滞后一个 reload 窗口",因为这层只挡危险启发,真边界仍是 fence)。**开放**:v1 先做 host preflight 还是先接受不覆盖?
- **R3 中介路径预算**:fs.read/write 的 32KiB/6s 需放宽;放宽多少、是否分级(文本 vs 二进制)待定。
- **R4 dill 快照大小与恢复时长**:崩溃恢复(唯一的 respawn 场景)仍走 kernel.dill;需实测大命名空间(数据帧等)的 snapshot/restore 耗时,决定是否设上限并提示模型。
- **R5 授权投放与 turn 生命周期的交错**:投放/回收发生在审批回调与模式切换时(可能在 cell 执行中),ACE 变更对正在打开句柄的可见性、与 `TurnInlineState` mid-turn 语义的对应需在 P2 细化(AGENTS.md L2-DES-CONV-002 的 promise matrix 需补 delivery 条目)。
- **R6 `.devo/config.toml` 项目级规则的信任边界**:项目文件里的 `PathPrefixPersist` 规则等同"仓库作者预授权"——建议项目级只允许 read 前缀,write 前缀仅用户级,首次使用时 UI 确认一次。**开放**:需要产品拍板。
- **R7 MCP 审批旁路(现网,fence 的前置收口项)**:`mcp.call` 中介 handler 存在但 Python 侧 kernel 内 `_registry` 本地直连(mcp.py:476-480),MCP 工具在不受限 kernel 里执行且零审批。P0 之前先决策:改走中介(host_request,现有 handler 即可用)还是等 fence 覆盖后重新定义 MCP 的审批语义(工具效果在 fence 内则按"围栏即授权"处理)。**开放**:产品口径。
- **R8 Windows 分叉落后上游**(2026-09-19 对照 codex v0.150.1):端口 env 线、继承 ACE 刷新震荡、glob 无界扫描、junction helper 查找、SEE_MASK_NOASYNC 等 6 项已修或待 re-vendor,清单与同步流程见 `crates/windows-sandbox/DEVO_PATCHES.md`。

---

## 附:与已删除原型(rlm-privilege-once)的映射(历史参考,文件已不存在)

| 原型(demos/rlm-privilege-once,已删除) | 本设计 |
|---|---|
| `Fence`(model.py:65) | `KernelFence` ← `SandboxProfile` 推导(§5.1) |
| `Grant` deny/once/session(model.py:23) | `ApprovalScopeValue` 8 scope 子集(§8) |
| once = host 代办 + `save_as` 回填 | `fs.read/write` 宿主代办路径(§6.1) |
| session = 扩根 + snapshot→respawn→restore | **否决**(破坏 kernel 状态保真);改为动态投放:ACE/代理策略/句柄,kernel 不重启(§9) |
| bwrap `--unshare-net` | 收敛到代理(WFP / 预连接 socket / Seatbelt)+ 策略默认 deny(§5.2,断网是策略状态而非进程属性) |
| HostVault | 不引入:once 代办已覆盖该场景,vault 会引入第二份状态 |
| adversary_suite / test_cases | 用例清单移入 §13-1,以本文档为准(源文件已删除) |
| `apply_network_policy` 的 socket 补丁 | **不采用**(P1:非边界);仅历史参考 |
