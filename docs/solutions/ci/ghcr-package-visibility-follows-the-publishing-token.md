---
title: "GHCR package visibility follows the token that created the package"
date: 2026-09-24
category: ci
module: GHCR package visibility
problem_type: documentation_gap
component: ci
severity: medium
applies_when:
  - "在 `docs/development.md` 或 README 里描述 GHCR 镜像的可见性、匿名拉取或首次发布准备步骤时"
  - "新增或改名容器镜像的包名，或考虑把推送凭据从 `GITHUB_TOKEN` 换成 PAT 时"
  - "评审发布链路中与镜像可见性、匿名拉取相关的检查时"
root_cause: inadequate_documentation
resolution_type: documentation_update
tags: [ghcr, github-packages, container-registry, visibility, anonymous-pull, github-token, release-automation, docs-drift]
---

# GHCR 包的可见性取决于创建它的 token 路径

## 背景

T9 的「发布验证」一节曾写着「首次发布后，在 GHCR package settings 中确认镜像可见性与仓库一致（public repository 对应 public package），否则匿名 `docker pull` 会失败」。这句话照搬了 GitHub 通用文档里的「新包默认 private」，隐含一个前提：本项目必须做一次人工设置才能得到可匿名拉取的镜像。

实测与该前提不符：`ghcr.io/geeknonerd/stuntdouble` 自 `v0.2.0` 首次推送起就可匿名拉取，包页面显示 `Public`；仓库里没有任何设置可见性的步骤，维护者也没有在 UI 里改过。

## 根因

GitHub 文档里有两条规则，看似矛盾，实际描述的是不同的创建路径：

- `data/reusables/package_registry/publishing-user-scoped-packages.md`（被 container registry 文档引用）：「When you first publish a package, the default visibility is private.」——适用于**在 workflow 之外**（CLI、PAT、个人账号命名空间）发布。
- `managing-github-packages-using-github-actions-workflows/publishing-and-installing-a-package-with-github-actions.md`：「by default if a workflow creates a package using the `GITHUB_TOKEN`, then: The package inherits the visibility and permissions model of the repository where the workflow is run.」——适用于**workflow 用 `GITHUB_TOKEN` 创建包**：公开仓库得到公开包。

本项目走的是第二条路径：`release-extras` job 带 `packages: write`，`docker/login-action` 用 `username: ${{ github.actor }}` 与 `password: ${{ secrets.GITHUB_TOKEN }}` 登录，包由该 token 创建（`.github/workflows/release-extras.yml`）。因此第一条规则在本仓库从未适用，把它写进文档是错的。

同一次核对确认的相邻事实：

- 只有 public package 允许匿名访问；private 或不存在的资源在 token 端点返回 `403` 与 `{"errors":[{"code":"DENIED","message":"requested access to the resource is denied"}]}`。
- 不带 `Authorization` 的 manifest 请求**永远**返回 `401` 加 `WWW-Authenticate`，所以「裸 `curl -I` 探测」判断不了可见性，必须走匿名 token 流程。
- 一旦设为 public 就不能改回 private（个人账号与组织账号的文档在这一点上一致），可见性不会悄悄回退。
- 组织还有一条 `Package Creation` 策略，决定成员新建包默认 public/private/internal；个人账号没有对应开关，因此对本仓库不适用。

## 做法

- `docs/development.md` 的可见性说明改为继承规则，并写明本项目没有需要人工维护的可见性步骤；只有改用 PAT/CLI 创建新包名时才需要人工改一次。
- 不在发布链路新增匿名拉取门禁：它验证的是 GitHub 的默认行为，不是我们的产物（issue #42 因此关闭，而不是实现检查）。若将来想留绊线，最低成本的形式是把 `release-extras` 里已有的 pull 检查换成空白 `DOCKER_CONFIG`，而不是新增 job 或定时任务。

## 验证

```bash
# 1. 匿名 token：拿不到（403 DENIED）就说明不可匿名拉取
curl -sS "https://ghcr.io/token?scope=repository:geeknonerd/stuntdouble:pull&service=ghcr.io" | jq -r .token

# 2. 用该 token 取 manifest：200 与 docker-content-digest 即匿名可拉
token=$(curl -sS "https://ghcr.io/token?scope=repository:geeknonerd/stuntdouble:pull&service=ghcr.io" | jq -r .token)
curl -sS -o /dev/null -D - \
  -H "Authorization: Bearer $token" \
  -H "Accept: application/vnd.oci.image.manifest.v1+json" \
  "https://ghcr.io/v2/geeknonerd/stuntdouble/manifests/v0.2.1"

# 3. 端到端：空白凭据目录（runner 上带凭据的 docker 会掩盖可见性问题）
DOCKER_CONFIG=$(mktemp -d) docker pull ghcr.io/geeknonerd/stuntdouble:v0.2.1
```
