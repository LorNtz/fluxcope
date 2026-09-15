# 首次发布：需要维护者完成的账号操作

仓库现已更名为 `LorNtz/fluxcope`，现已按维护者确认公开现有分支和历史。个人公开 tap `LorNtz/homebrew-tap` 已建立，仅含初始 README。首版仍严格以原 `master` 功能为准，不含 MCP。

## 已完成：创建两个 GitHub App

已确认源码发布 App 为 `fluxcope-release`（ID `4947987`），tap App 为 `fluxcope-tap`（ID `4948017`）。以下保留注册步骤，供核对和以后轮换使用。

这些是一次性账号设置。GitHub App 的注册、安装授权和私钥生成需要你在自己的 GitHub 页面完成。私钥只保留在本机或 GitHub environment secret 中，不要粘贴到聊天或提交到仓库。

### 1. 源码发布 App

打开 <https://github.com/settings/apps/new>，填写：

- **GitHub App name**：`fluxcope-release`。如果名称被占用，可以改名，完成后告诉我实际名称。
- **Homepage URL**：`https://github.com/LorNtz/fluxcope`。
- **Webhook → Active**：取消勾选。本项目不需要 webhook。
- **Repository permissions → Contents**：Read and write。
- **Repository permissions → Pull requests**：Read and write。
- Metadata 保持自动提供的 Read-only；其他权限保持 No access。
- **Where can this GitHub App be installed?**：Only on this account。

点 **Create GitHub App**。在设置页记下 **App ID**（不是 Client ID）。在 **Private keys** 区域点 **Generate a private key**，保存下载的 `.pem` 文件。

左侧点 **Install App → Install**，选择 **Only select repositories**，只选 `fluxcope`，完成安装。

### 2. Homebrew tap App

再次打开 <https://github.com/settings/apps/new>：

- 名称：`fluxcope-tap`（被占用时可以改名）。
- Homepage：`https://github.com/LorNtz/homebrew-tap`。
- 取消 Webhook Active。
- Repository permissions 仅给 **Contents: Read and write**，其余保持默认。
- Only on this account。

创建后记下 **App ID**，生成并保存在本机的 `.pem` 私钥。安装时只选 `homebrew-tap`。

完成后回复两个 **App ID 和实际 App 名称** 即可。不要发送 Client Secret、私钥内容、PAT 或 crates.io token。我会据此核对 bot 身份、配置受限环境，然后给出准确的私钥导入命令。

## 现在执行：导入 GitHub environment secrets

六个发布环境及对应分支/tag 限制现已建立，两个 App ID 已配置。现在可以执行下面的私钥导入命令。源码 App 私钥只进入 `release-prepare`、`crates-release` 和 `release-source`；tap App 私钥只进入 `homebrew-release`。这些环境会限制可以使用凭据的分支/tag，不另加每次发布的人工审批。

在终端输入本机私钥路径（路径可以包含空格；不要输入私钥内容）：

```bash
read -r -p "源码 App PEM 文件完整路径: " RELEASE_APP_KEY_PATH
for RELEASE_ENV_NAME in release-prepare crates-release release-source; do
  gh secret set RELEASE_APP_PRIVATE_KEY --repo LorNtz/fluxcope --env "$RELEASE_ENV_NAME" < "$RELEASE_APP_KEY_PATH" || { echo "导入失败，请停止并报告错误"; break; }
done
read -r -p "Tap App PEM 文件完整路径: " TAP_APP_KEY_PATH
gh secret set TAP_APP_PRIVATE_KEY --repo LorNtz/fluxcope --env homebrew-release < "$TAP_APP_KEY_PATH"
unset RELEASE_APP_KEY_PATH TAP_APP_KEY_PATH RELEASE_ENV_NAME
```

确认源码 App 已安装且仅选择 `fluxcope`，tap App 已安装且仅选择 `homebrew-tap`。完成四次 secret 设置后回复“私钥已导入”，我会只读核对 secret 名称并通过 CI 验证安装身份。

使用 Bash 执行上面的命令；macOS 默认 zsh 的 `read -p` 语义不同，可先运行 `bash`。命令只把私钥通过标准输入交给 GitHub CLI，私钥内容不会进入 shell 命令历史。

## 更后面：首次 crates.io 上传

现在还不要创建 crates.io token。需要先完成远端 CI、审核并合并准确的 `0.1.0` 发布 PR。到那时我会提供包含真实 PR、commit SHA、源码 CI 和包摘要的命令与链接。

首次上传成功后，在 crates.io 的 `fluxcope` crate 设置中配置 Trusted Publisher：owner `LorNtz`，repository `fluxcope`，workflow `release-plz.yml`，environment `crates-release`。一次性 token 在成功、失败或中断后都应撤销；后续版本无需再用永久 registry token。
