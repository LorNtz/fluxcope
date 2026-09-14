# Fluxcope v0.1.0 Release and CI Design — Revised

## 状态与范围

2026-09-14 修订，基于 [2026-09-03 设计及其 09-08 修订](2026-09-03-fluxcope-release-ci-design.md)。本文件是完整的新版设计，保留原文件用于对照；新增或修改设计文档不代表已实施 CI，也不授权执行发布、创建凭据、修改仓库可见性或重写历史。

本版优先减少维护者每次发布的操作，继续采用 release-plz + cargo-dist，保留“维护者合并 release PR 即授权发布”和 crate-first 发布顺序。自动化负责执行检查、准备版本、关联运行记录和整理证据。

**2026-09-14 实施决策更新：首版严格使用 `master` 现有 TUI/代理功能，不合入 feature 分支上的 MCP。为降低开发及持续维护成本，将 macOS 最低支持版本提高到 15，保留 Intel 与 Apple Silicon；取消 macOS 13 原生环境、Intel 云 Mac 租用和本地 Ventura VM 要求。** 两个架构分别使用 GitHub 标准托管 `macos-15-intel`、`macos-15` runner。公共仓库的标准 runner 免费；私有仓库受账户分钟额度和计费约束，不使用 `-large`/`-xlarge`。Linux 最低运行环境继续为 glibc 2.28。[GitHub runner 与计费范围](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)

**已确认的工具边界：cargo-dist 0.33.0 负责构建、archive、installer、formula、checksum 和 manifest；仓库维护薄层 workflow 的触发、门禁、权限与发布顺序。** 原生生成 workflow 的默认写权限及并行 plan 扩展无法表达本设计的授权顺序，因此不生成或修改其 CI 模板；`dist-workspace.toml` 使用 `ci = []`。

## 维护者的日常发布流程

对于已经合入 `master` 的内容，普通稳定版只需要三个步骤：

1. **审核 release PR。** 确认版本号、changelog 和兼容性影响。CI 自动完成构建、测试、包检查和发布预览。只有 TUI 交互发生变化时，才需要使用 CI 产物做相关人工体验检查。
2. **检查通过后合并 PR。** 使用 GitHub 的 squash merge，合并即授权该版本发布。不需要本地 checkout、编译、修改版本、执行 `cargo publish` 或推送 tag。
3. **查看统一的 Release result。** 结果自动关联 crates.io、GitHub Release 和 Homebrew。只有结果为 `complete` 才表示全部要求完成；失败时展示具体阶段及处理入口。

本地通过 `just release` 串联这三个步骤：脚本展示审核信息，等待一次明确的合并确认，再跟踪统一结果。GitHub 网页保留等价入口，两者使用同一套 PR、CI 和发布规则。

从本地 feature 分支开始时，维护者先提交希望发布的修改，再执行 `just ship`。脚本自动推送分支、创建或复用 feature PR、等待检查，在维护者确认合并功能后等待对应 release PR，继续完成版本审核、发布和结果跟踪。功能合入与版本发布分别审核，维护者无需手动串联 push、建 PR 和查询命令。

维护者不需要手动输入已发布版本、查找多个 run ID、复制 SHA、逐个下载资产验签或整理正常发布证据。相关检查仍然存在，由 CI 自动执行。

| 场景 | 人工入口 | 自动化负责 |
| --- | --- | --- |
| 提交 feature 供审核，暂不发布 | `just pr` | 推送已提交内容，创建或复用 feature PR，展示检查状态 |
| 从 feature 分支一路到发布 | `just ship` | 完成功能 PR 推送/创建、两阶段检查和审核衔接、发布及结果跟踪 |
| 普通稳定版 | `just release` 或网页审核并合并现有 release PR | 提议版本、测试、发布、结果汇总 |
| 指定或修正版本 | `just release-prepare 0.2.0` 或 Actions → Prepare release | 更新可审核 PR，刷新 lockfile/changelog，重跑检查 |
| GitHub-only RC | `just release-prepare 0.2.0-rc.1` | 准备 RC PR；合并后自动创建 RC tag 并分发 |
| RC 转稳定版 | `just release-prepare 0.2.0` | 生成 promotion PR，整理稳定版 changelog |
| 临时基础设施失败 | Release result 中定位失败运行，重跑失败 job | 保持原始 SHA/ref，重新验证结果 |
| crate 已发布但后续未完成 | `just release-recover 0.2.0` 或 Actions → Recover release | 验证当前状态，恢复缺失 tag、分发启动或 tap 阶段 |
| 首次创建 crate | 一次性的 bootstrap 流程 | 预检查、上传后验证、创建首个 tag、分发和汇总 |

Prepare release 只准备 PR，不发布。Recover release 只能继续已有授权版本，不构成新的发布授权。

## 本地操作入口：just

扩展仓库现有 `.justfile`：从 feature 到发布使用 `just ship`，只提交功能供审核使用 `just pr`，发布已合入主干的内容使用 `just release`。just 负责命令发现、参数传递和帮助说明；版本管理、构建和实际发布仍在 CI 执行。以下是待实施的命令契约，不表示当前 `.justfile` 已提供这些 recipes。[just 官方文档](https://just.systems/man/en/)

### 命令约定

| 命令 | 行为 |
| --- | --- |
| `just` | 默认显示命令及简短说明，不默认构建或发布；同时支持 `just --list` |
| `just pr` | 推送当前 feature 分支的已提交内容，创建或复用目标为 `master` 的 feature PR，展示链接和检查状态；不合并或发布 |
| `just ship` | 复用 pr 流程，等待 feature CI 和功能合并确认，再等待包含此次功能的 release PR，复用 release 流程完成发布 |
| `just release` | 查找当前发布 PR，展示审核信息，等待检查，通过维护者确认后合并并跟踪结果 |
| `just release-prepare [version]` | 调用远端 Prepare release；不填版本时刷新提议，填写时指定版本；只准备 PR，不合并 |
| `just release-status [version]` | 查看并跟踪统一结果；省略版本时优先选择唯一进行中/未完成版本，否则显示最近发布结果 |
| `just release-recover <version>` | 读取失败阶段，展示将执行的恢复动作，确认后调用对应恢复入口或重跑指定失败 job |
| `just doctor` | 检查 just、Git、GitHub CLI、Python 版本、GitHub 登录、仓库身份及所需 workflow 是否存在 |

保留现有 build/run 等开发命令的职责；发布入口不把它们作为本地前置依赖。每个公开 recipe 写明一句用途，runbook 提供 shell 补全的可选配置，不再另设一组含义相同的短别名。

从当前 feature 发布时只需先完成本地 commit，再执行 `just ship`；功能已经合入主干时执行 `just release`。需要指定版本、准备 RC 或 promotion 时使用：

```bash
just release-prepare 0.2.0-rc.1  # 只准备 RC PR
just release                     # 审核、合并并跟踪 RC 发布

just release-prepare 0.2.0       # 只准备稳定版 promotion PR
just release                     # 审核、合并并跟踪稳定版发布
```

### pr：推送功能并创建审核入口

命令要求当前处于可识别的 feature 分支，拒绝 detached HEAD、`master` 和保留的发布准备分支。校验目标仓库、push remote 与目标分支，获取必要远端引用，展示将推送的已提交变更范围及当前 head SHA。无需事先配置 upstream 或手动 push。

仅推送已提交内容，不自动执行 `git add .` 或 commit。有未提交、暂存或未跟踪内容时列出其未被包含的状态；继续操作不改变这些内容。远端分支不存在时创建并建立对应关系；分支已存在时只允许正常的 fast-forward push。远端领先、历史分叉或存在合并冲突时给出诊断，不自动 force-push、rebase 或解决语义冲突。

推送成功后，按准确 head repository/branch 和 `master` 查询开放 feature PR。唯一匹配时复用并保留已有人工标题和正文；没有匹配时创建。新 PR 根据提交信息提议 Conventional Commit 标题和说明，并允许维护者编辑，不把最后一条提交信息无条件当作整个功能的说明。不直接把 `gh pr create` 的隐式 fork/push 作为 remote 选择机制，明确指定 head/base。[GitHub CLI 创建 PR](https://cli.github.com/manual/gh_pr_create)

命令输出 PR 链接、实际推送的 head 和检查状态后结束。再次执行会更新同一开放 PR 的代码，不重复创建 PR；已关闭或已合并的 PR 不能作为开放 PR 复用。网络或查询失败不解释为“PR 不存在”。

### ship：从 feature 分支串联到发布

`just ship` 复用 pr 与 release 的实现，避免维护第三套推送、合并或发布规则。完整流程为：

```text
维护者提交准备发布的修改
→ just ship
→ 推送当前 feature 分支，创建或复用 feature PR
→ 等待 required CI，展示准确 feature PR 差异
→ 维护者审核并确认合并功能 PR
→ 等待 bot 创建或刷新包含本次功能的 release PR
→ 展示完整版本、changelog 和 release PR 差异
→ 维护者审核并确认发布
→ 自动跟踪所有适用渠道的结果
```

两个确认分别代表“该功能可以合入主干”和“该完整版本现在可以发布”。功能 PR 检查通过后才按已审核 head 执行 squash merge，沿用 required checks 和 `--match-head-commit`；版本发布仍使用后文 release 的确认，不用前一次功能合并确认代替。两阶段都不使用管理员 bypass 或无交互的隐式同意。[GitHub CLI 检查等待](https://cli.github.com/manual/gh_pr_checks)

功能合并后记录准确 merge SHA。只有证明 release PR 的源码历史已经包含该 merge SHA，才进入版本审核；不能抓取一个尚未刷新、遗漏本次功能的 release PR 就发布。必要时调用不改变版本意图的 Prepare release 刷新，等待结果并重新验证。release PR 可以同时包含其他已合入功能，审核界面必须展示完整发布范围，不宣称只发布当前 feature。

如果合入的是不产生 package 变化的文档等内容，bot 可以合法地不创建 release PR。此时明确报告“功能已合入，当前没有可准备的发布”，而非伪造发布成功或制造空版本。准备被部分发布状态阻塞、需要显式 RC/promotion 版本或需要版本修正时，给出准确的 status/recover/prepare 命令，保留已经完成的功能合并结果。

任何等待期间 feature head 或 release head 改变，都使对应旧审核失效。功能已经合并而发布尚未确认时中断，不回滚功能，也不自动完成版本发布。恢复时重新核对 GitHub 的功能 PR、merge SHA、release PR 和发布结果，再继续尚未完成的阶段。

### release 的交互与授权

脚本校验目标仓库后，通过 GitHub 查询符合来源、目标分支、label 和 intent 要求的发布 PR，展示仓库、版本/渠道、PR、准确 head SHA、changelog、差异链接及适用人工体验检查状态。版本和 SHA 来自远端 PR，不取本地 `Cargo.toml` 或当前分支推断发布目标。

只有一个候选时自动选择；多个候选时展示列表让维护者选择，不静默取最新项。没有 PR 时展示 Prepare release 的结果/入口及下一条命令，不自动制造空版本、改变版本策略或绕过 PR 发布。

确认检查通过且维护者完成审核后，显示一次合并确认，例如：

```text
仓库：LorNtz/fluxcope
发布：0.2.0（stable）
PR：#128
Head：<本次展示并审核的准确 SHA>
检查：全部通过
变更：<changelog 和差异链接>

合并此 PR 将开始发布 0.2.0。确认合并？[y/N]

发布已启动，正在跟踪结果……
✓ crates.io
✓ GitHub Release
✓ Homebrew
发布完成：<Release result 链接>
```

此确认就是“合并即授权发布”的本地表达，不增加合并后的第二次发布批准。未确认不执行合并；没有交互终端时不隐式回答 yes。脚本使用 `gh pr merge --squash --match-head-commit`，锁定刚刚展示和审核的 head，不使用 `--admin` 绕过规则，也不预先开启延迟自动合并。[GitHub CLI merge](https://cli.github.com/manual/gh_pr_merge)

等待期间发生 bot 更新、PR 替换或 head 变化时，重新展示变更并重新审核；不能把旧确认用于新 head。合并后从 PR 获取准确 merge SHA，按该发布身份跟踪统一报告，直到 `complete`、`rc-complete` 或需要维护者处理的状态。单个 source workflow 成功不等于整个发布成功。

### 自动发现、可恢复性与工作目录

所有 GitHub 操作显式指定 `LorNtz/fluxcope`，校验本地 repository 配置/remote，避免在 fork 或其他仓库中误执行。PR、版本、SHA 和 run/attempt ID 自动查询；只有候选不唯一时才请求选择。查询报错与“没有候选”分别处理。

`just release` 及 release-prepare/status/recover 操作远端已审核内容，不 checkout、stash、commit、push 本地代码，也不要求当前工作树干净。`just pr` / `just ship` 则会 fetch/push 当前功能的已提交内容，并可建立 upstream；它们仍不修改工作树、暂存区或本地提交历史，不自动切换/删除当前分支。交互明确区分当前推送的功能与最终发布的完整远端版本。首次 bootstrap 的本地上传仍遵守专用干净 checkout 例外，不隐藏在普通 `just release` 或 `just ship` 中。

关闭终端、Ctrl-C 或本地网络中断只停止本地等待，不取消远端发布。再次执行 `just release-status` 即可重新发现并跟踪准确发布；已经发布或正在发布时，`just release` 引导查看现有结果，不重复合并、派发或上传。恢复依据 GitHub 的权威状态，不依赖本地缓存中的 PR/run ID。

`just ship` 在功能阶段中断后，可从相同 feature 分支再次执行。通过 head repository/branch、准确提交和 GitHub PR 关联识别进度：功能未合并时复用开放 PR，已合并时继续对应版本阶段。squash merge 后不能仅用 feature tip 是否为 `master` 祖先判断功能已合入；应读取真实 PR merge SHA。多个历史 PR 无法唯一关联时请求选择；若本地有新的提交，展示为新增工作，不沿用旧 head 的审核。发布已完成则返回既有结果，不创建新的空 PR 或版本。

失败输出包含当前阶段、远端链接和一条可直接执行的后续命令，例如 `just release-recover 0.2.0`。recover 先展示实际动作及准确版本，再复用既有恢复策略；遇到源码或完整性冲突停止自动恢复，不盲目重跑所有成功 job，不自动 yank 或改变 tag。

### 实现边界与依赖

使用两层结构：

```text
.justfile             命令、参数和帮助说明
scripts/release.py    分支推送、PR 创建/复用、交互、head 校验、合并、状态跟踪和恢复调度
```

Python 使用标准库处理 JSON 和进程调用，通过 `gh` 的结构化输出访问 GitHub。以参数数组调用子进程，禁用 shell 拼接；just 参数通过安全的位置参数或环境变量传入脚本，不能将版本、PR 内容或接口输出插入待执行的 shell 文本。

本地只要求兼容版本的 just、Git、GitHub CLI、Python 3.11+ 及已登录的维护者账户；pr/ship 还需要通过配置的 remote 推送功能分支的权限。实施时记录并验证最低工具版本，优先使用现有 just 1.43.0 支持的稳定语法。普通发布无需在本地安装 release-plz/cargo-dist，也不配置 App 私钥、tap token 或 registry token。`just doctor` 只诊断并给出缺失依赖/登录的修复命令，不自动安装工具、修改认证或打印 secret。

本地 wrapper 仅提供方便的客户端流程，CI 仍独立校验发布来源、版本及权限，网页入口同样有效。共用可复用的规则和报告 schema，避免本地再实现一份 SemVer/changelog/打包逻辑。

## 相对旧设计的主要调整

| 原设计 | 本版 |
| --- | --- |
| 每次发布前本地构建并运行完整人工 smoke 清单 | 自动化应用验收；人工检查限于变化的 TUI 体验 |
| 人工查询两个 workflow、版本和多个摘要 | 单一 Release result，自动记录和归档证据 |
| 本地需要记忆 GitHub CLI 参数、PR 和运行查询命令 | `.justfile` 提供固定命令，`just release` 串联审核、合并与结果跟踪 |
| feature 推送、建 PR 与后续发布分开手动串联 | `just pr` 提交审核，`just ship` 自动衔接功能 PR 与包含其变更的 release PR |
| 本地执行版本修正、RC 准备和 promotion | Prepare release 统一入口，持久记录显式版本意图 |
| RC 需要维护者手动推送 tag | RC PR 合并后由受限 CI job 自动创建 tag |
| macOS 15 双架构测试及旧系统 Homebrew 安装均为发布硬门槛 | 保留 macOS 15 双架构自动运行门槛；Homebrew 安装环境单独定义 |
| 每个普通 PR 都解包后再次 `cargo install` | 保留 `cargo package`；额外安装验证集中到发布及打包规则变更 |
| tag-push 分发，另需定制 dispatch 恢复入口 | cargo-dist 使用原生 dispatch 模式，上游自动派发；恢复复用该入口 |
| 固定 `SHA256SUMS` 文件名及大量自定义内部步骤 | 使用 cargo-dist 原生输出和扩展点，验收资产覆盖及执行结果 |

不再引入第二个版本管理器、第二个 changelog 生成器或另一套二进制打包工具。

## 产品与发布契约

产品名为 **Fluxcope**，所有机器标识统一为 `fluxcope`：crate、Rust crate 引用、可执行文件、Clap identity、产物前缀、Homebrew formula、默认状态目录 `~/.fluxcope` 。

公开仓库为 `LorNtz/fluxcope`，tap 为 `LorNtz/homebrew-tap`，首次版本为 `0.1.0`。原设计中的命名排查仅是历史初筛，实施 bootstrap 时重新确认 crates.io 名称可用性；网络查询失败不能解释为名称可用。

分发渠道包括 GitHub Releases、shell installer、Homebrew 和 crates.io。v0.1.0 不增加 Windows、系统原生 Linux 软件包、容器分发、自动更新、macOS 签名或 notarization。

发布工作不改变代理 bind 默认值、认证机制或 ACL。Fluxcope 仍是会生成本地 CA 的 MITM 代理，默认监听全部 IPv4 接口；公开说明必须清楚告知网络暴露、私钥保护、流量敏感性及 CA 清理方式。

## 工具职责与工作流边界

| 职责 | 唯一负责人 |
| --- | --- |
| 版本提议、Cargo metadata、lockfile、committed changelog | release-plz |
| 批准具体版本及源码 | 维护者合并经过检查的 release PR |
| 稳定版 crate 上传和稳定版 tag | release-plz |
| RC tag、bootstrap/缺失 tag 恢复 | 有明确渠道校验的薄层 orchestration |
| 四平台 archives、installer、checksums、provenance、GitHub Release、formula 生成 | cargo-dist |
| 应用与安装验收 | 可复用测试和验证 job |
| 状态关联和维护者结果页面 | 只读发布观察器及受限报告写入 |
| 本地命令、交互与远端操作调度 | `.justfile` + `scripts/release.py`，复用上述 CI 接口 |

预计维护以下入口；验证实现放入共享脚本或 reusable workflows，避免在每条工作流中重复规则：

| 文件 | 触发条件 | 作用 |
| --- | --- | --- |
| `.github/workflows/ci.yml` | PR、`master` push | 源码质量、测试、package、发布元数据检查 |
| `.github/workflows/release-plz.yml` | `master` push、手动 Prepare release | 更新发布 PR；符合条件的合并执行稳定版 publisher 或 RC tag job |
| `.github/workflows/release.yml` | PR、自动/手动 `workflow_dispatch` | 自有薄层 workflow，调用 cargo-dist 生成预览及正式分发产物 |
| `.github/workflows/release-status.yml` | 指定 workflow 的运行事件、只读手动刷新 | 更新统一 Release result，关联证据 |
| `.github/workflows/release-recovery.yml` | 手动 Recover release，限 `master` | bootstrap 收尾、缺失 tag、分发启动、tap 恢复 |

这些文件名是实施接口。`release-plz.yml` 作为 crates.io Trusted Publisher 的固定 workflow identity，重命名前必须同步修改 Trusted Publisher 配置。

### 分发触发方式

采用自有 workflow 的 `workflow_dispatch` 入口。上游创建并验证 tag 后自动派发 `release.yml`，ref 和 `tag` 输入均指向该版本；维护者无需第二次点击发布按钮。预览由 PR 事件触发，不执行发布写入。

Fluxcope 对该入口增加限制：正式分发只接受**已经存在、与已审核 PR 和准确源码 SHA 对应的 tag**。不允许 cargo-dist 为一个任意输入隐式创建新发布 tag；`dry-run` 和 PR 只运行不发布的预览。

GitHub 支持使用具有 `actions: write` 权限的工作流 token 派发 `workflow_dispatch`。此路径不依赖普通 token 写 tag 后能否触发另一个 push workflow。GitHub App 仍用于创建可直接获得 CI 的 release PR 和执行受 tag 规则约束的写入。[GitHub workflow 触发规则](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)

## Prepare release 与版本意图

### 自动提议

普通 `master` push 触发 release-plz 更新一个待审核稳定版 PR。没有可发布的 package 变化时可以不产生 PR。自动更新串行执行，不覆盖正在执行的显式版本准备操作。

当已有未完成或部分发布的版本时，暂停新的发布准备，并给出旧版本的结果链接。RC 期间暂停普通稳定版自动提议，由维护者使用 Prepare release 请求下一个 RC 或 promotion。

### 手动入口

Prepare release 仅从 `master` 运行，输入为可选 `version`：

- 留空：刷新自动提议；不清除已存在的显式版本覆盖。
- `X.Y.Z`：准备或修正稳定版；若当前处于 RC，则生成 promotion PR。
- `X.Y.Z-rc.N`：准备 GitHub-only RC，`N` 为正整数。

其他版本格式、build metadata、已发布或已使用的版本、相对上个正式版本不合理的倒退均拒绝。不通过 shell 字符串拼接执行用户输入。

整个操作仍通过 release-plz 的 update/set-version 能力修改版本及 changelog，只刷新 workspace lockfile，不顺便升级第三方依赖。随后由 App 更新或新建 PR；结果直接提供 PR 链接。[release-plz set-version](https://release-plz.dev/docs/usage/set-version)

### 显式版本覆盖的持久性

每个发布 PR 带有 `.github/release-intent.json`，记录 schema version、渠道、版本模式（automatic/explicit）、目标版本和上一个稳定版。它不进入 `.crate` 或二进制 archive。

准备 wrapper 在执行 release-plz 之前读取**当前开放的、已验证来源的发布 PR**中的意图，在生成之后重新应用并验证。即使 release-plz 替换了 PR，也必须把同一意图带到新 PR。显式版本一直保持到该发布合并、关闭或被另一次显式请求替换；不能因为 bot 刷新而悄悄恢复自动提议。

已合并到 `master` 的历史 intent 只描述对应发布，不自动继承给下一版本。PR body、标题或 label 仅帮助导航，均不能单独授权发布。实现使用 release-plz 原生 `update` / `set-version` 计算版本和 changelog，由薄层 wrapper 以准确 head lease 提交这四份元数据并创建或更新 PR。中断在 push、建 PR、加 label 之间时可继续；结果 tree 不变时保留现有 head 和检查。原生 `release` 仍负责后续 Trusted Publishing。版本计算在无发布凭据的只读 job 中执行，产出的四个限定文件及准确 base/head 作为有界数据交给独立 PR 写入 job；写入 job 不执行 Cargo 或上游 artifact 中的程序。失败重跑只在远端完整 tree 与原计划一致且 ancestry 保持时继续收尾，不接受其他变更。

全部准备操作共用 concurrency group。捕获 PR head 后再更新；存在非预期人工修改或来源冲突时停止并显示差异，不强制覆盖。PR head 改变后需要新的检查和针对该 head 的审核。

### 分支与 changelog

稳定版分支使用 `release-plz-` 前缀；promotion 也使用该前缀。RC 分支使用 `rc/X.Y.Z-rc.N`。只有同仓库、目标为 `master`、由配置中的 App 或维护者准备、标记为 `release` 的 PR 才可能成为发布 PR。

稳定版 changelog 比较基准是上一个稳定 tag。RC 不改变该基准；promotion 按前一稳定版到当前源码重新整理合并后的说明，避免重复 RC 条目。首次版本不引用不存在的前一版本。

配置至少明确 `release_always = false`、`git_release_enable = false`、`git_tag_name = "v{{ version }}"`、`dependencies_update = false`，保持 package publish verification。锁定准确 release-plz 版本并验证配置 schema。Git-cliff 使用其内置集成，不单独安装另一个 changelog 工具。[release-plz 配置](https://release-plz.dev/docs/config)

## CI 与发布前检查

### 固定工具链与依赖策略

沿用原设计的 Rust 1.98.0 pin，提交 `rust-toolchain.toml`，包含 rustfmt/clippy；Cargo 声明 `rust-version = "1.98"`。实施时验证该精确版本和四目标构建可用性，不静默退回或跟随 `stable`。

release-plz、cargo-dist、cargo-deny 和历史 secret scanner 均固定精确版本；Action 固定完整 commit SHA 并注释版本。下载的工具验证 checksum，registry 安装固定版本和锁文件。Dependabot 每周分组更新 Cargo 和 Actions；工具链、发布工具及镜像 digest 更新作为独立审核变更。

`deny.toml` 规定 advisory、许可、registry/Git source 及有实际影响的重复依赖策略。临时例外记录原因和到期条件；不再并列运行没有额外价值的 cargo-audit job。

### 普通 PR 与 master push

默认 `contents: read`，无发布凭据，执行：

| 稳定检查名 | 检查内容 |
| --- | --- |
| Source quality | `cargo fmt --all -- --check`、严格 locked Clippy、`cargo deny check` |
| Tests (Linux) | `cargo test --all-targets --all-features --locked`，包含适用的自动 smoke |
| Tests (macOS) | 同一 locked 测试命令，包含适用的自动 smoke |
| Package | metadata/include 校验、`cargo package --locked`、生成配置一致性和 release intent 校验 |
| PR title | Conventional Commit 标题，只在 PR 事件要求 |

push publisher 等待前四项在**准确事件 SHA**上的结果，不等待一个只在 PR 事件存在的 PR title check。缺失或失败不能当成功；不会改用当前 `master` tip 的绿灯。

缓存只提升速度，不跳过正确性检查。checkouts 固定事件 commit，发布及 changelog 分析按需获取完整历史，禁用持久 checkout credentials。

### 安装验证与预览产物

`cargo package` 自身会解包并验证构建，因此普通 PR 不再额外重复解包后安装。release PR、Cargo/package include、构建及安装规则变化时，Package job 额外执行从 `.crate` 解包后的 `cargo install --locked --path`，检查安装后的准确版本及 help。正式发布后另做一次 exact-version registry 安装。[Cargo package 行为](https://doc.rust-lang.org/cargo/commands/cargo-package.html)

自有 `release.yml` 对发布分支 PR 运行四平台 cargo-dist 构建，并上传可下载预览；每个 PR 都产生固定 `Release preview` 汇总 check，普通功能 PR 可明确跳过分发构建。预览只有只读仓库权限，不创建 tag 或 Release。平台列表与 `dist-workspace.toml` 的 targets 通过检查保持一致，archive 与 installer 布局仍由 cargo-dist 生成。

合并发布 PR 前必须通过四目标分发预览和对应验证。将稳定的 `Release preview` 汇总名纳入 required checks，升级工具时同步验证分支规则，避免漂移导致永久等待。

预览记录 PR head、base 和实际测试的 merge ref/SHA。预览不能替代正式 tag 产物：合并后按授权源码重建，重新生成 provenance 并验收。对普通 PR 不要求维护者下载这些产物。

## 自动化应用验收

测试接收显式的待测可执行文件路径，复用同一 fixture/harness 验证测试构建、预览 archive 和正式 archive，避免只测试工作树程序。

| 行为 | 自动验证方式 |
| --- | --- |
| 首次启动与私有状态 | 在独立测试路径启动，验证目录 `0700`、私钥等敏感文件 `0600` |
| 启动、端口占用及退出 | 使用受控端口与子进程，验证错误、正常退出及无残留 listener |
| HTTP/HTTPS capture | 本地固定 request/response fixture，验证 captured URL、headers/body 和实际转发 |
| recording、map-remote、map-local | 固定规则和请求，验证记录开关及有效映射结果 |
| 配置持久化 | 通过已有路径配置隔离状态，重启后核对设置 |
| CA 下载 | 请求临时下载端点，验证仅返回证书、无私钥材料 |
| TUI 导航、搜索、正文查看 | 复用现有 PTY 基础发送输入，断言确定性的状态或输出 |
| 终端恢复 | 用可查询终端状态的 PTY harness 比较正常退出及启动失败前后的终端状态 |

HTTPS 客户端只在测试进程中显式信任临时 CA，不写入开发机或 runner 的系统信任库，不使用跳过证书验证代替正确的测试链路。所有流量为合成数据。测试路径、端口和超时明确可控；成功、失败、超时都终止测试子进程并清理自己的文件。

普通 CI 持续验证应用行为；正式 archive 至少执行准确 help/version、关键 HTTP/HTTPS/mapping和 PTY 启停检查。平台相关的权限和终端行为在相应平台运行。缺失的自动验收必须在 release automation 验收前补齐，不能永久退回每版全量人工清单。

人工只负责视觉表现、键位体验、剪贴板交互等自动检查难以充分覆盖的变化。首发进行一次完整体验检查；后续在 TUI/相关依赖或交互行为变化时执行相应项目，记录所测 PR head 和产物。通过审核确认无相关变化的发布不要求重做人工 smoke。

## 平台与安装渠道

### 二进制运行契约

| Target | 最低运行环境 | 正式产物执行验证 |
| --- | --- | --- |
| `aarch64-apple-darwin` | macOS 15 | native Apple Silicon `macos-15` |
| `x86_64-apple-darwin` | macOS 15 | native Intel `macos-15-intel` |
| `aarch64-unknown-linux-gnu` | glibc 2.28 | 对应架构的 glibc 2.28 userspace |
| `x86_64-unknown-linux-gnu` | glibc 2.28 | 对应架构的 glibc 2.28 userspace |

macOS 构建显式设置 `MACOSX_DEPLOYMENT_TARGET=15.0`，检查 Mach-O 最低系统 load command。两个架构的实际 archive 分别在标准 `macos-15`、`macos-15-intel` runner 上执行，并校验真实 OS 主版本、CPU 架构及未使用 Rosetta。所有运行均隔离 HOME 和测试流量，不信任 runner 名称代替实际系统检查。

不再提供 macOS 13/14 的二进制兼容承诺，不保留付费旧系统验证作为首版发布门槛。GitHub 将来退役 macOS 15 runner 时，重新决策提高最低版本或恢复旧系统测试环境，不隐式改为 `macos-latest`。

Linux 使用 cargo-dist 支持的 container/custom-runner 设置和固定 digest 的 glibc 2.28 构建环境，优先 native x86_64/arm64 runner；不自建一套交叉编译分发工具。发布前检查所需 `GLIBC_*` 符号不超过 2.28，并在对应 userspace 运行。仅配置最低版本 metadata 不足以证明实际依赖符合该要求。

### Homebrew 与其他安装渠道

二进制最低运行环境和 Homebrew 的支持环境分别定义。四个架构的 formula 都引用已发布 archive；Homebrew 安装测试在锁定的、可运行当前 Homebrew 的 macOS/Linux 环境执行，不要求在 glibc 2.28 容器中安装整个 Homebrew。macOS 13/14 不属于首版支持范围。

Homebrew 的平台支持策略由上游决定；本项目可以测试其生成 formula，但不将上游不支持的组合宣传为 Homebrew 官方支持。旧系统用户根据二进制运行契约选择 archive、shell installer 或适用的源码构建方式。[Homebrew 安装要求](https://docs.brew.sh/Installation#macos-requirements)、[Linux 行为](https://docs.brew.sh/Homebrew-on-Linux)

所有稳定版验证 exact-version Cargo 安装和隔离 shell 安装。Homebrew 验证记录实际 formula commit 与版本；测试发现已被更新的 formula 时不能误报旧版本成功。RC 不执行稳定渠道安装。

macOS 产物未签名、未 notarize。README 和 release notes 在安装说明附近注明该状态、checksum/provenance 验证方式，以及针对已验证文件的有限 Gatekeeper 操作，不建议全局关闭 Gatekeeper。

## 稳定版发布与源码身份

`release-plz.yml` 的发布 job 仅接受 `LorNtz/fluxcope` 的 `master` push，并在申请 registry 凭据前验证：

1. `CRATES_BOOTSTRAPPED` 严格等于 `true`；manifest 为稳定 `X.Y.Z`。
2. 事件 SHA 是已合并、同仓库、目标为 `master` 的合格 release PR 的 squash commit；分支、作者、label、intent 和 Cargo 版本相符。
3. checkout 固定为该 SHA；Cargo、lockfile、changelog 版本一致，工作树干净。
4. 该 SHA 的 required push CI 成功，执行 locked package preflight，记录 `.crate` 摘要。
5. 没有另一个未处理的发布阻塞当前版本。

之后调用 release-plz 的 native Trusted Publishing，以 App token 创建稳定 tag，并验证 registry checksum 和 tag peeled commit 均匹配预期。成功后自动派发 cargo-dist。普通合并、准备 dispatch、RC、缺失或畸形 bootstrap 标志均不能进入稳定 publisher。

保留 `release_always = false`，但不把分支前缀识别当作全部授权机制。回归场景必须包含 release PR 合并后另一普通 PR 立即合并，证明上传和 tag 始终对应原授权 SHA。release-plz 的 squash commit 选择行为需要针对锁定版本验证。[源码选择说明](https://release-plz.dev/docs/usage/release#what-commit-is-released)

publisher concurrency 不取消正在上传的运行。不同普通 push 可以被排除，但不能依赖 GitHub concurrency 自动队列完整保存每个发布事件；发布 PR 合并策略和 release readiness 在未完成版本存在时阻止后续发布。

crates.io 与 GitHub/Homebrew 不是原子事务。crate 成功而后续失败属于 `partial`，不能通过撤销源码版本或移动 tag 回滚。release-plz no-op 也不证明 tag 和其他渠道完整。

## RC 与稳定版 promotion

RC 由 Prepare release 生成 PR，经同样源码、package、预览和适用人工体验检查后 squash merge。

独立 RC tag job 验证准确合并 SHA、intent 和 `rc/X.Y.Z-rc.N` 分支，创建该准确源码的 tag，再自动 dispatch cargo-dist。该 job 没有 registry OIDC 权限，不调用稳定版 `release-plz release`，也不更新 Homebrew。

cargo-dist 校验 prerelease 标记并发布 GitHub-only 产物；release 必须为 prerelease 且非 latest，沿用四平台及完整性检查。RC 成功后结果为 `rc-complete`，crates.io/Homebrew 显示 `not-applicable`，而非伪造成功。

promotion 使用 Prepare release 指定稳定版本。CI 重新生成以此前稳定版为基准的 changelog，并明确将要包含的全部变更；如果 RC 后还合入了其他代码，它们必须出现在审核范围中。合并后走普通稳定 publisher，产生新的稳定 tag 和产物，不重命名 RC tag，也不把 RC archive 原样改名充当稳定版。

## cargo-dist 分发与扩展方式

维护薄层 `release.yml` 的 job DAG，不再依赖 cargo-dist 生成 CI 的扩展点。固定 cargo-dist 版本及配置，以 `dist plan`、`dist build --artifacts local/global` 调用原生能力；不重新实现 archive、installer 或 formula 生成。

已核对 0.33.0 源码：原生 workflow 顶层默认 `contents: write`，custom plan job 与内置 plan 并行。自有 workflow 改为默认 `contents: read`，先执行独立授权 gate，再运行只读构建/验证，最后由单独 job 申请最小发布权限：

- 计划及未授权预览没有发布凭据。授权 gate 检查正式 dispatch 的 tag 已存在、ref/input 相符、tag 未移动、准确源码及合并 PR 有效；stable 还验证 registry checksum。
- 该 gate 必须阻断所有构建后的发布写入。不能仅因为 custom job 名称叫作 plan/host，就假定它先于内置 job 执行。尤其不能让内置 plan 阶段在检查前具有创建未授权 tag/release 的能力。
- 正式 tag 的四平台构建、archive 解包、应用 smoke、最低系统检查和完整性检查必须全部完成后才能发布 GitHub Release。
- GitHub Release 先创建 draft，上传完整资产和证据，再发布为 immutable release。
- Homebrew 更新必须晚于 GitHub 资产可访问；tap 测试失败时不撤回 GitHub Release。

自有 workflow 必须通过权限和依赖顺序回归检查，工具升级时重新验证 CLI、manifest、installer 与 formula 的接口。[GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)

### 资产与验证

使用 cargo-dist 的 archive 布局、原生统一 checksum 文件及 `dist-manifest.json`，不固定要求文件必须名为 `SHA256SUMS`。正式结果至少覆盖四个 archives 和 shell installer；checksum 清单不覆盖自己。若锁定版本只对 archives 生成 checksum，则通过扩展补齐 installer，不重新实现 archive 打包。[原生 checksums](https://axodotdev.github.io/cargo-dist/book/artifacts/checksums.html)

四个 archives 与 installer 都需要 build-provenance attestations，绑定准确 tag、source SHA 和文件 digest。配置其覆盖范围及阶段，不能假设默认只在本地产物阶段生成的 attestation 已覆盖后生成的 installer。发布观察器记录实际 signer workflow identity，独立验证按该 identity 校验。

shell installer 必须验证下载的 archive；cargo-dist 0.33.0 缺少 `sha256sum` 时会跳过检查，因此在生成脚本开头添加一个受版本约束的短 preflight：macOS 使用系统 `shasum -a 256` 兼容，两个工具都缺少时直接停止。其余安装逻辑保留原生输出；扩展后的脚本纳入总 checksum 和 provenance。用损坏下载 fixture 验证 checksum 失败会阻止安装，不能只根据工具文档推定已启用。Homebrew formula 的 SHA-256 来自实际发布资产。

release notes 从对应 committed changelog 及固定安装/安全说明生成。用于验证和恢复的公共元数据、formula snapshot 和摘要作为辅助资产保留；其文件名来自 manifest/报告。只有必要且明确命名的资产进入正式 Release。保留原生 archive `.sha256` sidecar，避免 manifest 引用失效；另外保存 `source-evidence.json`，使源码与 crate 摘要在 CI artifact 过期后仍可复核。

draft 阶段可以重建并替换资产，但必须重新验证整组 checksum 与 provenance。published 阶段禁止替换、删除或上传不同字节的同名资产。

## Release result：一个结果入口

每个版本以 `(repository, version, source SHA)` 唯一关联，输出人类可读 summary 和有 schema 的 `release-report.json`。报告至少包括：PR、准确 merge SHA、tag、渠道、各次 workflow/run attempt、crate digest、资产 digest、平台结果、安装结果、tap commit、适用人工体验记录和失败阶段。

| 总状态 | 意义 |
| --- | --- |
| `preparing` / `running` | 准备或发布阶段仍在执行 |
| `bootstrap-required` | 首个 crate 尚未建立，未执行自动上传 |
| `failed` | 尚无公开渠道成功，但执行已失败 |
| `partial` | 至少一个渠道已发布，其他要求失败或尚未完成 |
| `complete` | 稳定版所有渠道、平台、完整性与安装要求均验证通过 |
| `rc-complete` | RC 的 GitHub-only 要求全部完成 |

细分阶段可显示 `awaiting-distribution`，但缺失运行不能自动算成功。发布方在 dispatch 后通过 ref/version/SHA 和预期 workflow ID 查询启动结果，有界重试并记录派发失败；不让未启动的任务无限隐藏在绿色源发布结果后。

### 报告的产生与可信来源

统一入口是 source commit 上的 `Release <version>` check，其详情链接指向最新汇总。该结果不会作为本次 release PR 合并前的 required check，避免先发布才能合并的循环依赖。PR 提供预览入口；合并后可从准确 merge commit 进入最终结果。

`release-status.yml` 订阅指定发布/恢复 workflow 的启动与完成事件，在失败、取消和部分完成时也进行汇总；另提供不触发发布副作用的手动刷新入口。报告 job 按每个 producer job 的真实执行 attempt 关联证据，支持只重跑失败 job；源 CI 正常等待时显示 running。报告 job 使用 GitHub API 和已验证来源的机器报告，不运行上游 artifact 或不受信任 checkout 中的代码。

观察器校验 workflow ID、仓库、事件/ref、source SHA、run attempt 和 schema 后才采用生产者证据；不能按最新一次同名运行或 PR 提供的 URL 决定发布成功。通过 registry 和 Release API 复核公开事实，检查链路失败时保持不完整状态。

报告更新按版本串行合并。旧的完成事件不能覆盖较新 attempt 的结果；某个渠道的成功不能丢失另一渠道的失败。原始阶段报告分别保留，`release-report.json` 是可重新生成的汇总，不引入独立数据库。

普通元数据和合成测试结果可公开随版本保留；secret 扫描原始报告、私有历史发现及敏感日志存放私有位置，只在公开结果中保留检查状态。发布前的源码、资产和验证摘要作为 immutable Release 的资产保存。发布后才产生的 Cargo/Homebrew 最终结果不能补传到已冻结的资产集合：由独立的结果归档 job 在受限 `master` 环境重取实时证据，将包含版本、digest、tap commit 的 canonical JSON 通过 `release-status.yml` 签名，再把显示副本写入仍可编辑的 Release notes。未来观察必须验证该准确 JSON 的 attestation、受信 workflow、`refs/heads/master` 和 GitHub 托管 runner；可编辑 notes 本身不能证明检查成功。已有有效归档不反复签名写入；归档开始前 check 保持可操作 partial，客户端仅在该 observer 仍运行时等待，job 启动失败或取消不会留下永久 running。临时构建保留 7 天，源码 `.crate` 保留 30 天，安装/结果报告保留 90 天；保留历史 CI run 元数据用于核对源码检查。过期后的原始日志不再宣称仍可下载，资产本身仍可独立验签。

## 凭据、分支与仓库规则

`master` 保持 squash-only、required checks、禁止直接/强制 push 和删除；release PR 必须跟上当前 `master`。不要求第二个人审批，不开启 release PR auto-merge。维护者的合并是唯一日常发布授权，不叠加例行环境审批。

GitHub App 仅安装于发布仓库，授予必要 Contents/PR 权限，不授予 `master` bypass。tag creation 例外与 tag update/delete 禁止规则分离；App 可以创建经过验证的 tag，但不能移动已有 tag。

| job/环境 | 可用权限与凭据 |
| --- | --- |
| PR/source CI/preview | 只读，无 App 私钥、registry 或 tap 凭据 |
| Prepare release | 计算 job 只读；独立 PR 写入 job 使用 App token，环境只允许 `master` |
| Stable publisher / `crates-release` | App token、native crates.io OIDC；环境只允许 `master` |
| RC tag / `release-tags` | App token；只允许 `master`，没有 registry OIDC |
| Distribution dispatcher | `actions: write`，只派发经验证的准确版本 |
| GitHub asset/provenance jobs | 按 job 分配 `contents: write`、`id-token: write` 和 `attestations: write` |
| Tap updater / `homebrew-release` | 只限 tap contents 的 `HOMEBREW_TAP_TOKEN`；只允许受保护版本 tag，job 拒绝 RC |
| Recovery coordinator / `release-tags` | 根据操作按需获取 tag token/actions 写权限；不上传 crate |
| Result observer | `actions: read`、`contents: read`、`checks: write`；无发布 secret |
| Result archiver | 独立 `release-result` 环境：`id-token/attestations: write` 签名 canonical 结果，`contents/checks: write` 更新既有 notes 和统一 check，不操作 tag/资产 |

App ID 使用 `RELEASE_APP_ID`，private key 仅放到确实需要它的受限环境中，统一记录轮换要求。所有 installation token 短期签发，用完撤销。tap token 单独限权，不扩大 App 到跨仓库权限来省略一次性设置。

tap 正常发布和恢复共用 `publish-homebrew.yml` 的更新/验证实现：正常流程由分发 workflow 在 GitHub Release 发布后调用，恢复协调器在准确 tag ref 上 dispatch 它。该 reusable workflow 也提供受同一授权检查保护的 dispatch 入口，使 `homebrew-release` 环境不会错误地在 `master` ref 上被调用。使用 cargo-dist 生成的 formula，薄层 job 负责受限的跨仓库提交、安装验证及幂等恢复，不重新实现 formula 生成。

crates.io Trusted Publisher 绑定 `LorNtz/fluxcope`、`release-plz.yml`、`crates-release`。不保存永久 registry token，也不同时运行另一套 crates.io auth action。`workflow_run` 报告器永远不把 PR 代码作为有写权限的程序执行。

## 首次公开与 bootstrap

### 一次性设置

完成 identity 切换、metadata/许可、公开文档和 CI；启用适用的 GitHub secret scanning、私有漏洞报告、immutable releases、分支/tag/environment 规则。先创建 App、tap 及只限必要仓库的凭据，初始 `CRATES_BOOTSTRAPPED=false`。验证标准托管 Intel/Apple Silicon macOS 15 执行环境，记录实际 runner、container digest、工具 pin 及轮换方式。

在改变仓库可见性之前审查当前树和完整 Git 历史：tokens、cookies、私钥、CA、captures、日志、运行描述符、私有主机与个人数据、禁止公开的工作文档和大型生成资产。使用固定版本 gitleaks 等支持全历史扫描的工具，并执行人工 MITM 数据检查。报告和细粒度 allowlist 私下保留。

`requirements.md`、`what_i_just_did.md` 保持 local-only，历史中存在时也必须在公开前处理；`agent_journal.md` 继续作为跟踪文件，公开前同样检查内容。当前 ignore 不能证明历史安全；先撤销泄露凭据，再在明确授权下处理历史与可见性。

### 首个 crate 的唯一人工上传

Trusted Publishing 当前不能创建新 crate，因此初次上传仍需维护者的一次性 registry token。这是账号引导限制，不能通过把 RC 或占位包先上传来掩盖。[release-plz bootstrap 限制](https://release-plz.dev/docs/github/quickstart)

1. 通过普通发布 PR 准备并合并 `0.1.0`。CI 完整验收，publisher 显示 `bootstrap-required` 并保留期望源码和 `.crate` digest，不自动创建 tag。
2. 执行 `just release-bootstrap 0.1.0 <真实 PR 号>`。helper 创建准确合并 SHA 的干净专用 checkout，运行 locked package、准确 `.crate` 安装及 help/version 检查，验证本地 digest 与 CI 相符后才请求 token。初次体验检查使用 CI 产物完成。
3. 确认名称、owner、版本及源码后，用仅足够创建该 crate、尽可能短期的 token 执行一次 `cargo publish --locked`。token 不写入文件、shell history、GitHub secret 或 `cargo login`；无论成功、失败或中断均到 crates.io 撤销。
4. 在 crates.io 配置上述 Trusted Publisher。
5. 运行 `just release-recover 0.1.0`，自动关联原 release PR 并展示准确的 `tag-and-distribute` 操作。自动验证合并身份、registry checksum、原始报告和 tag 状态，然后创建缺失的准确首个 tag 并自动启动分发。
6. 统一结果确认 GitHub/Homebrew 及安装验证完成后，将 `CRATES_BOOTSTRAPPED` 设为 `true`。该一次性仓库变量变更仍由维护者执行，不为此给常规发布 App 仓库管理权限。

上传响应丢失时先只读检查 registry。既有 digest 不一致立即停止，不重复上传、不改 tag、不把它当作幂等成功。

## 恢复与不可变发布

Recover release 接受明确操作、版本及原 PR；从 `master` 的受信任控制代码执行，但所有源码检查、package 重建和正式分发使用该版本原始授权 SHA/tag。恢复记录标识当前协调器版本与原始发布源码，不声称修复后的控制代码属于旧 tag。

| 故障 | 恢复动作 |
| --- | --- |
| 无 release PR | 查看 Prepare release 结果；无 package 变化可以是正常 no-op |
| 上传前 CI/OIDC/preflight 失败 | 修复可恢复的环境问题，重跑原始 publisher；源码缺陷通过新审核变更处理 |
| crate 已上传、tag 缺失 | 验证原始 PR/SHA、干净 package digest、registry 相等及 tag 确实不存在，再由受限 job 创建缺失 tag |
| tag 已存在、分发尚未启动 | 校验准确 tag 和源码后，复用原生 dispatch 入口启动 `release.yml` |
| draft 阶段构建或验证失败 | 重跑原运行的失败 job，保持 SHA/ref，重验完整 draft 资产组 |
| GitHub 已发布、Homebrew 失败 | 单独恢复 tap job，读取已发布且已验证的资产/formula；不重跑 GitHub 上传 |
| 仅状态报告失败 | 只读刷新报告，不重新发布任何渠道 |
| 任一已有 tag、crate、资产 digest 与预期不同 | 停止恢复，作为完整性冲突处理 |
| 已发布版本存在源码缺陷 | 发布经过审核的新版本；仅在有害且明确授权时 yank |

GitHub 的失败 job 重跑保留原始 SHA/ref；它有平台规定的可重跑时间限制，不能把它作为无限期恢复存储。[GitHub rerun](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs)

恢复协调器先检查渠道状态。已发布 GitHub Release 的版本不能进入会覆盖其资产的分发流程；只允许只读验证或独立 tap 修复。`release.yml` 的 `verify` 模式负责公开后网络故障恢复，不重建、不上传、不申请 attestation 写入权限；成功的只读验证证据也能完成 RC 或稳定版结果。已有 draft 可从同一 tag 恢复，已有进行中 run 则返回其链接。重复 dispatch 必须受发布互斥保护；未完成版本不会并行执行两次上传。对同一版本的操作可重复检查，不能重复制造外部副作用。

bootstrap/缺失 tag 恢复和 RC tag creation 共用准确源码验证与 create-if-absent 逻辑；一致的已有 tag 可继续，冲突 tag 绝不覆盖。源码或 tagged workflow 缺陷需要不同提交字节时，准备新版本，不改旧 tag 中的 workflow。

损坏的 notes 归档只有在全部实时渠道证据独立齐全时才能自动重新签名；缺少证据不能信任或修补成功字段。Homebrew 已有更高版本时，旧版恢复查找准确匹配的历史 formula commit 进行安装验证，不降级 tap。

部分发布会阻止准备下一版本。若旧版本因源码缺陷必须由新版本替代，`just release-replace <version>` 提供明确的“标记为待替代”分类操作；该运行名为 Classify，不计作发布活动，避免自身观察器改变原状态造成竞争，保留 `partial` 状态及原因，允许准备后续修复 PR；不会把旧版本改成成功或自动 yank。下一版本结果链接回被替代版本。

## Identity、Cargo package 与公开文档

### 一次性命名切换

覆盖 Cargo 名称、Rust 引用、CLI help/version、TUI/log identity、默认配置/证书/日志/runtime/lock/socket 路径、测试、bench imports、文档及安装命令。

旧 `.wirelens` 数据保持不动，不读取、迁移、合并或删除。无别名可执行文件、fallback 目录、兼容 URI 或 deprecated crate alias。公开安装和运行接口不得保留旧名。

扫描 active source、配置、工作流、公共使用说明、package 和 release assets 中的旧标识；Git 历史与保留的历史设计记录单独审查，不把本设计描述旧命名的历史文字误认为运行时兼容接口。包和产物仍排除设计文档。

### Cargo package

`Cargo.toml` 包含 `name = "fluxcope"`、初始 `version = "0.1.0"`、edition 2024、上述 rust-version、`MIT OR Apache-2.0`、description、repository/homepage、README、keywords/categories 和明确 include allowlist。

allowlist 仅包含构建、理解及许可 package 所需的 manifests/lockfile、Rust sources、README、changelog 和两份许可证。Cargo 生成的 normalized manifest、`Cargo.toml.orig`、`.cargo_vcs_info.json` 独立验证。排除证书/私钥、运行状态、配置、captures/logs、本地任务记录、bench 输出、设计规划及构建产物。

CLI version 取自 Cargo metadata；`fluxcope --version` 和 `--help` 无需进入 TUI，首版不包含 `fluxcope mcp`；MCP 以后通过独立功能 PR 引入。release profile 保留 panic unwinding 以支持终端恢复和正常停机，strip/thin LTO 仅在验证后启用。

### 文档结构

README 描述功能、四平台/渠道支持、unsigned macOS、验证与安装、CLI、CA 信任和移除、主要交互及默认全部 IPv4 监听。强调仅用于可信网络或受 host firewall 约束的环境。

SECURITY.md 使用私有漏洞报告，解释 CA 私钥权限、流量中的凭据、未认证代理暴露。CONTRIBUTING.md 描述工具链、CI、Conventional Commit/squash 和禁止提交敏感捕获数据。

`docs/releasing.md` 首页呈现“提交修改 → `just ship`”与“已合入主干 → `just release`”两条路径、`just pr` 的单独审核入口，以及本地/网页入口表；其后分别放置本地工具准备与 `just doctor`、两阶段审核和中断恢复、Prepare release、条件性人工体验、首次 setup/bootstrap、恢复分类、独立验证命令、证据保留及凭据轮换。独立 checksum/attestation/registry 安装命令用于复核与故障处理，不再次列为每版人工必做项。

## 实施顺序与验收

按可审核的工作组实施：identity/metadata/许可；自动化应用验收；普通 CI 与 cargo-dist 预览；Prepare release 与源码 publisher；受限分发和恢复；统一结果、just 本地入口与公开 runbook。应用缺陷在独立可识别变更中修复，不顺带改代理行为或无关架构。

交付前必须演示：

- 普通 PR 无发布 secret；App PR 无需维护者额外批准才能开始正常 CI。
- 普通稳定版由审核、合并、查看结果完成，维护者无需本地构建或手动 tag。
- `just` 默认展示帮助；`just doctor` 正确识别工具/登录/仓库问题，且不修改环境或认证。
- `just release` 从准确远端 PR 展示信息，只有明确确认后才按审核过的 head 合并；多候选不误选，head 变化不复用旧确认，失败检查不被绕过。
- `just pr` 首次推送并建立 feature PR；再次执行复用准确开放 PR，不覆盖人工说明，不误把查询失败当作 PR 缺失。Conventional Commit 标题可审核修改。
- `just ship` 在 feature PR 和 release PR 两处分别等待检查、展示差异并取得对应合并确认；head 改变时不能沿用旧审核。
- 功能合入后 `just ship` 等待包含准确 feature merge SHA 的 release PR；过期 PR、无 package 变化及其他版本阻塞均给出正确结果，不发布遗漏功能或空版本。
- 在有未提交修改的开发分支执行 release 系列命令不改变本地 Git 状态；pr/ship 只推送已提交内容及更新必要 remote/upstream 元数据，不修改工作树、暂存区或提交历史。错误仓库、保留分支、分叉 push 被拒绝。
- `just ship` 在 push、PR 创建、功能合并及发布各阶段中断后都可基于远端状态继续；squash 合并的功能不会被重复提交，新增本地提交不会复用旧审核。
- Ctrl-C、网络中断后可以用 `just release-status` 恢复跟踪；重复执行不会重复派发发布，失败信息提供准确版本的下一步命令。
- `just release-prepare` 仅准备 PR，RC/promotion 复用相同参数入口；`just release-recover` 只调度已授权版本的明确恢复动作。
- 显式版本在普通 bot 刷新、PR 替换和重复 Prepare release 后保持正确；第三方依赖不被附带升级。
- 准备 dispatch 不上传，普通合并不发布，RC 无 registry/tap 凭据，promotion 生成新的稳定 tag。
- PR head 变化会失效相关检查；另一 `master` 合并不会改变发布源码。
- 精确 package 源码构建、条件性安装验证、四目标预览、正式 archive smoke 与最低系统运行检查全部通过。
- Intel 与 Apple Silicon macOS 15 标准托管执行环境可用；失联、OS/架构不符或运行失败均不能绕过二进制发布门槛。Homebrew 验证独立使用适用系统。
- 错误版本、缺失 tag、ref/input 不同、未授权 PR 和错误 registry digest 均在发布写入前被拒绝。
- 人为损坏 archive 时 installer 拒绝安装；四个 archives 及 installer 的 checksum/provenance 覆盖完整。
- GitHub 发布前全量验证；发布后资产不再修改；Homebrew 始终引用可访问且准确版本的资产。
- 模拟 crate 成功/二进制失败、缺失 tag、派发失败、tap 失败、reporter 失败和重复恢复，结果分类正确且不重复上传。
- `workflow_run` 收到错误来源、过期 attempt 或不可信 artifact 时不执行其代码，也不污染最终结果。
- 正常发布证据自动归档；原始私密扫描报告不会进入公共 Release。
- 首次 bootstrap token 在所有结果下撤销，Trusted Publisher 配置正确，bootstrap flag 只在全渠道完成后启用。
- 全历史安全检查、公开文档、rename/package 扫描和适用人工体验检查完成。

**完成标准：工具仍各自负责擅长的工作。发布已合入内容时，维护者做版本与兼容性判断和一次发布合并授权；从 feature 开始时，再增加对应功能 PR 的审核合并，push、建 PR、等待和阶段衔接均自动完成。正常发布、RC、版本修正及常见恢复均有明确入口和可验证结果。**
