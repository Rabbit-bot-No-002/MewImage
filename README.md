# MewImage

当前版本：`v1.1.0`

一个可多设备同步的图片生成平台。
AI Coding 的产物，主要是满足个人使用需求，Docker部署。

- 前端：`Leptos CSR`
- 后端：`Axum + SQLite`
- 资源存储：`本地文件存储`或 `S3 兼容对象存储`
- 模式：本地优先，登录后手动跨设备同步

## 视频演示
bilibili：https://www.bilibili.com/video/BV1njKa6VEm8/?share_source=copy_web&vd_source=81866f08e909626b4220c9656edc09ce  

youtube：https://youtu.be/43DXly6Cw5U?si=57ihgJLuHs3uyjgu

## 当前实现

- GPT Image 2.5 基础适配：可选 `gpt-image-2.5-flare` / `gpt-image-2.5-sunburst`，质量支持 `auto/low/medium/high/xhigh/max`
- 历史、会话、收藏、参考图、界面偏好默认保存在浏览器 IndexedDB
- 登录只是同步增强能力，不是使用前提
- 注册用户默认需要管理员审批，审批通过后才能使用云端同步和服务器资源存储
- 注册与登录带设备/IP 限流；账号连续输错 5 次密码后锁定
- 用户界面会显示当前账号保存在服务器的图片数量；管理员列表会显示每个用户的服务器图片数量
- 支持 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容`
- 管理员可从顶栏盾牌入口进入独立管理后台，分页管理用户、执行本页批量操作、导出安全 CSV，并查看永久审计日志
- 管理员可创建“托管账号”并在服务器端为其分配多条服务商配置。托管账号拥有已审批普通用户的功能；浏览器、同步快照和工作区 ZIP 均不会收到上游地址、API Key、密文或 Key 提示，只能切换管理员允许的配置与模型。管理员还可建立加密服务商模板，批量分配并按需同步到关联账号
- 提供API 原生 / 本地去背 两种背景模式；中转站不支持官方透明参数时，可在浏览器中生成真正带 Alpha 通道的 PNG 或 WebP
- 代理生图使用后台任务和轻量状态轮询；上传参考图后会立即释放浏览器准备预算，结果就绪时再按 FIFO 领取处理预算
- 支持本机记忆的生图队列模式：提交后画廊立即显示等待卡，可继续修改提示词、参考图和参数并发提交；队列模式与连续修改模式互斥
- 可切换主题
- 可上传全局自定义背景，并调整填充、位置、透明度、模糊度、主题遮罩和背景图层；还可降低页面卡片不透明度，让背景保持可见
- 数据管理支持完整工作区 ZIP、单会话项目包导出与恢复、合并导入、分类清除，以及登录用户自助查看和清除自己的云端数据
- 新增“模板广场”主视图：公开模板支持搜索、多标签筛选、最新/热门排序、游客点赞、外链分享、收藏快照和一键填入工作台；创建和管理模板仅向管理员开放

## Docker Compose 一键部署

Docker Hub 镜像：[`mewlab/mewimage`](https://hub.docker.com/r/mewlab/mewimage)

服务器只需要安装 Docker 与 Docker Compose，下面的命令会下载 Compose 和环境变量模板：

```bash
mkdir -p mewimage && cd mewimage

curl -LO https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/docker-compose.yml
curl -o .env https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/.env.example
```

国内镜像加速
```bash
mkdir -p mewimage && cd mewimage

curl -fL -O https://gitee.com/ln-q/MewImage/raw/main/docker-compose.yml
curl -fL -o .env https://gitee.com/ln-q/MewImage/raw/main/.env.example
```

生成认证密钥和管理员初始化口令：

```bash
openssl rand -hex 32
openssl rand -base64 32
```

编辑 `.env`，至少修改以下三项：

```dotenv
MEW_AUTH_SECRET=第一条命令生成的随机值
MEW_ADMIN_TOKEN=第二条命令生成的随机值
MEW_ALLOWED_ORIGINS=https://你的正式域名
```

如果暂时通过 `http://服务器IP:3188` 直接访问，则把 `MEW_ALLOWED_ORIGINS` 改成该完整来源，并保持：

```dotenv
MEW_HOST_BIND=0.0.0.0
MEW_SESSION_SECURE=false
MEW_TRUST_PROXY_HEADERS=false
```

如果已经通过 Nginx Proxy Manager、Nginx 或 Caddy 配置 HTTPS，则推荐：

```dotenv
MEW_ALLOWED_ORIGINS=https://你的正式域名
MEW_SESSION_SECURE=true
MEW_TRUST_PROXY_HEADERS=true
MEW_TRUSTED_PROXY_CIDRS=反向代理所在的精确IP或CIDR
```

只有当 MewImage 后端端口无法绕过反向代理直接从公网访问时，才能开启 `MEW_TRUST_PROXY_HEADERS`，并且必须用 `MEW_TRUSTED_PROXY_CIDRS` 限定真实反向代理；反向代理还必须主动覆盖 `X-Real-IP` 和 `X-Forwarded-For`，不得原样透传客户端提交的同名请求头。其他连接提交的转发头会被忽略。

容器使用固定的非 root 用户 `10001:10001`。Linux 或 NAS 在首次启动前必须主动创建可写的数据目录，不能依赖 Docker 自动创建 `./data`，否则该目录通常会属于 `root:root`。从旧版 root 容器升级时，也必须先执行 `docker compose down`，再做一次递归权限迁移：

```bash
chmod 600 ./.env
sudo mkdir -p ./data/assets ./data/.tmp
sudo chown -R 10001:10001 ./data
sudo find ./data -type d -exec chmod 750 {} \;
sudo find ./data -type f -exec chmod 600 {} \;
```

Windows 和 macOS Docker Desktop 的 bind mount 通常不需要手工修改 UID/GID；如果启动日志提示 `/data` 无写入权限，再检查目录共享和 ACL。

如果容器反复重启且日志出现 `Permission denied (os error 13)`，OpenResty/Nginx 的 `502 Bad Gateway` 只是后端未能启动的连带结果。按上面的命令修正实际挂载到 `/data` 的宿主机目录后重新启动即可，不要把应用改回 root 用户。SELinux 主机还需为 bind mount 添加私有标签 `./data:/data:rw,Z`；CIFS、NFS 或 NAS 共享则需要在共享 ACL 或挂载参数中授予 UID/GID `10001` 写权限。

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs -f app
```

Compose 默认仅绑定宿主机 `127.0.0.1:3188`，适合宿主机上直接运行的 Nginx 或 Caddy。运行在另一个容器中的 Nginx Proxy Manager 无法直接访问宿主机回环地址；应通过 Compose override 加入共享 Docker 网络并直接代理 `app:3000`，或只向受防火墙保护的地址发布端口。需要直接通过服务器 IP 访问时，把 `.env` 中的 `MEW_HOST_BIND` 显式改为 `0.0.0.0`。SQLite 和登录同步图片统一保存在当前目录的 `./data`：

```text
mewimage/
├── docker-compose.yml
├── .env
└── data/
    ├── mew-image.db
    └── assets/
```

首次打开页面后，注册第一个账号，并在折叠的管理员初始化入口填写 `MEW_ADMIN_TOKEN`。第一个管理员建立后，普通用户注册会进入待审批状态。

如需使用托管账号，先在 `.env` 中设置 `MEW_MANAGED_PROVIDER_SECRET`。它必须是 `openssl rand -hex 32` 生成的 64 位十六进制值，并应与 SQLite 数据库一同固定备份。没有任何托管配置时缺少该密钥不会影响普通部署启动；一旦数据库已有托管配置，密钥缺失、错误或无法解密都会阻止启动，避免带着不可用凭据继续运行。密钥丢失后无法恢复原有 API Key，只能从数据库备份和对应密钥一起恢复。

管理员从顶栏盾牌按钮进入 `#/admin` 管理后台；“托管账号”和“服务商模板”使用独立侧栏入口。创建托管账号时必须选择服务商模板或手工保存第一条配置。系统生成的临时密码只展示一次，托管用户首次登录必须修改密码；管理员重置密码会立即使旧会话失效。托管配置可逐项维护，也可选择协议和接口模式一致的多条记录批量更新地址和/或 Key；Key 留空表示保留旧值，保存后仅显示末尾提示且不能回显或复制。服务商模板修改后不会自动影响账号，需由管理员明确选择关联账号执行同步。

说明：

- 默认拉取 `mewlab/mewimage:latest`，可以通过 `.env` 中的 `MEW_DOCKER_IMAGE` 固定具体版本。
- 当前 Docker Hub 镜像提供 `linux/amd64` 架构；ARM64 服务器或 NAS 需要后续发布多架构镜像。
- 默认启用 Local 图片存储，不需要额外部署 S3，登录同步图片会写入 `./data/assets`。
- 云端图片默认单文件不超过 64 MiB、每个已审批用户不超过 5 GiB；达到配额后仍可读取、下载和删除已有资源，但不能继续新增。
- 默认设置 `1 GiB` 容器内存上限，可以通过 `MEW_MEMORY_LIMIT` 调整。
- 游客每 IP 默认最多同时运行 4 个生成任务和 2 个结果图代理请求；10 分钟内默认分别允许 30 次和 120 次请求。
- 公网第三方中转站不需要加入白名单，但必须解析到安全公网地址且默认使用 HTTPS；高级 `CustomHttp` 始终需要已审批账号。
- 生图任务提交后由浏览器短轮询结果，通常不需要为 Nginx Proxy Manager 单独放大数分钟的读取超时。

## 本地开发

1. 安装依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
```

2. 准备环境变量

```bash
cp .env.example .env
```

3. 启动后端

```bash
cargo run -p mew-image-backend
```

4. 启动前端开发服务器

```bash
cd frontend
trunk serve --open
```

说明：

- 推荐优先通过后端地址 `http://127.0.0.1:3000` 使用完整功能。
- 如果只打开静态文件或只开 `trunk serve`，纯本地 UI 可以运行，但某些代理相关能力会受限。
- 若要让本地后端访问同一台电脑上的明文图像上游，请先停止后端，再根据系统在启动后端的同一个终端中设置以下临时环境变量。

Windows PowerShell：

```powershell
$env:MEW_ALLOW_HTTP_UPSTREAM = "true"
$env:MEW_ALLOW_LOOPBACK_UPSTREAM = "true"
cargo run -p mew-image-backend
```

Linux/macOS Bash 或 Zsh：

```bash
export MEW_ALLOW_HTTP_UPSTREAM=true
export MEW_ALLOW_LOOPBACK_UPSTREAM=true
cargo run -p mew-image-backend
```

Linux/macOS 也可以只对本次启动设置变量：

```bash
MEW_ALLOW_HTTP_UPSTREAM=true \
MEW_ALLOW_LOOPBACK_UPSTREAM=true \
cargo run -p mew-image-backend
```

该开发开关只允许显式的 `127.0.0.1`、`::1`、`localhost` 和 `localhost.localdomain`，且仅在 MewImage 后端自身监听环回地址时生效；`10.x`、`172.16–31.x`、`192.168.x` 等局域网地址仍会被拒绝。默认 Docker 监听 `0.0.0.0:3000`，因此该开关不会放宽容器部署。直接执行 `cargo run` 不会自动读取项目根目录的 `.env`，使用上述命令最可靠。测试结束后可关闭当前终端；PowerShell 也可执行 `Remove-Item Env:MEW_ALLOW_LOOPBACK_UPSTREAM, Env:MEW_ALLOW_HTTP_UPSTREAM`，Bash/Zsh 可执行 `unset MEW_ALLOW_LOOPBACK_UPSTREAM MEW_ALLOW_HTTP_UPSTREAM` 清除临时变量。


#### 本地生图被 SSRF 策略拦截

`MEW_ALLOW_HTTP_UPSTREAM` 只允许明文 HTTP，`MEW_ALLOW_LOOPBACK_UPSTREAM` 只允许显式环回上游。生图还会下载上游返回的图片 URL，并逐次检查图片重定向；因此即使 API 是公网 IP，图片域名解析到私网或代理软件的 Fake-IP（例如 `198.18.x.x`）仍会失败。错误中的目标主机和后端日志中的解析地址可用于定位实际被拦截的地址。

本地需要完整测试这些上游时，先停止后端，在**启动后端的同一个 PowerShell 终端**执行：

```powershell
Set-Location D:\project\MewImage
$env:MEW_LISTEN = "127.0.0.1:3000"
$env:MEW_ALLOW_HTTP_UPSTREAM = "true"
$env:MEW_DEV_BYPASS_UPSTREAM_SSRF = "true"
cargo run -p mew-image-backend
```

Linux/macOS 可在项目根目录运行：

```bash
MEW_LISTEN=127.0.0.1:3000 \
MEW_ALLOW_HTTP_UPSTREAM=true \
MEW_DEV_BYPASS_UPSTREAM_SSRF=true \
cargo run -p mew-image-backend
```

前端仍可使用 `http://127.0.0.1:8080/`。在前端终端设置变量不会影响已经启动的后端，直接 `cargo run` 也不会自动加载 `.env`。启动日志会明确显示开关是否生效。

`MEW_DEV_BYPASS_UPSTREAM_SSRF` 默认关闭，仅在后端绑定环回地址时生效；绑定 `0.0.0.0`、`[::]` 或局域网地址时会忽略此开关并记录警告。开启后，生成请求、图片下载和图片重定向均跳过主机名与 IP 地址的 SSRF 拦截，无需再设置 `MEW_ALLOW_LOOPBACK_UPSTREAM`；HTTP 开关、URL 协议/凭据检查、已启用的主机白名单、大小/超时/重定向次数限制仍保留。这个模式允许访问本机和内部服务，仅用于本机开发，不应通过反向代理或隧道对外公开。

测试后关闭该终端，或执行 `Remove-Item Env:MEW_DEV_BYPASS_UPSTREAM_SSRF, Env:MEW_ALLOW_HTTP_UPSTREAM` 并重启后端，即恢复默认策略。

### 账号规则

- 普通用户注册后状态为 `pending`，可以登录查看状态和修改密码，但不能使用云端同步或服务器资源存储。
- 管理员在顶栏盾牌入口的独立管理后台批准用户后，用户状态变为 `approved`；设置弹层仅保留个人设置。
- 管理员可以禁用或恢复用户；后端会阻止管理员禁用当前登录的自己，避免误锁。管理员也可在二次确认后为任意账号生成一次性临时密码，旧会话会立即失效，用户下次登录必须改密。
- 管理员可永久删除普通用户及其服务器图片、同步快照、模板和账号；浏览器本地数据不受服务器删除影响。
- 注册和改密都要求强密码：至少 10 位，并同时包含大写字母、小写字母、数字和符号，且需要二次确认。
- 同一设备默认最多成功注册 3 个账号；注册和登录还会按真实客户端 IP 限流。
- 账号连续输错 5 次密码后默认锁定 5 分钟，成功登录后自动清零失败计数。
- 用户名由数据库唯一索引强制去重，注册界面也会提前检查用户名是否可用。
- 初始化口令默认折叠；只要系统尚无管理员就显示入口。即使服务器漏配 token，入口也不会静默消失，提交后会显示明确错误。
- 系统已有管理员后，初始化入口永久隐藏且后端拒绝再次初始化。


### Local 迁移到 S3

当前版本不会在切换 `MEW_ASSET_STORE` 时自动搬迁文件，也没有 Local/S3 双读回退。直接从 `local` 改成 `s3` 前，必须先将本地对象复制到 S3；否则 SQLite 中的图片索引仍然存在，但程序会在新 S3 Bucket 中找不到对应文件。

本地和 S3 使用相同对象键：

```text
users/{user_id}/assets/...
```

因此不需要修改 SQLite，也不需要通过网页重新导出、导入图片。推荐迁移步骤：

1. 停止 MewImage，避免迁移期间继续产生新图片或未完成上传。
2. 备份整个 `./data` 目录，包括 `mew-image.db` 和 `assets/`。
3. 将 `./data/assets` 中的内容同步到 S3 Bucket 根目录，保留原始相对路径。
4. 确认 Bucket 中的路径直接以 `users/` 开头，而不是 `assets/users/`。
5. 保留原 SQLite 数据库，配置 S3 环境变量并将 `MEW_ASSET_STORE` 改为 `s3`。
6. 重启后检查历史图片读取、新图片上传、删除和跨设备同步。
7. 确认运行稳定后再清理本地图片；建议至少保留一段时间作为回滚备份。

AWS CLI 或多数 S3 兼容服务可使用：

```bash
aws s3 sync ./data/assets s3://你的Bucket \
  --endpoint-url https://你的S3端点
```

MinIO Client 可使用：

```bash
mc mirror ./data/assets 你的别名/你的Bucket
```

迁移失败时，只要本地文件和 SQLite 仍保留，将 `MEW_ASSET_STORE` 切回 `local` 并重启即可回滚。
