# MewImage

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

- 历史、会话、收藏、参考图、界面偏好默认保存在浏览器 IndexedDB
- 登录只是同步增强能力，不是使用前提
- 注册用户默认需要管理员审批，审批通过后才能使用云端同步和服务器资源存储
- 注册与登录带设备/IP 限流；账号连续输错 5 次密码后锁定
- 用户界面会显示当前账号保存在服务器的图片数量；管理员列表会显示每个用户的服务器图片数量
- 支持 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容`
- 登录态远程资源上传可使用本地文件存储；需要云对象存储时可切换到 S3 兼容模式
- 数据管理支持本地 ZIP 导出、合并导入、分类清除，以及登录用户自助查看和清除自己的云端数据

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
MEW_SESSION_SECURE=false
MEW_TRUST_PROXY_HEADERS=false
```

如果已经通过 Nginx Proxy Manager、Nginx 或 Caddy 配置 HTTPS，则推荐：

```dotenv
MEW_ALLOWED_ORIGINS=https://你的正式域名
MEW_SESSION_SECURE=true
MEW_TRUST_PROXY_HEADERS=true
```

只有当 MewImage 后端端口无法绕过反向代理直接从公网访问时，才能开启 `MEW_TRUST_PROXY_HEADERS`；否则客户端可以伪造来源 IP，绕过注册和登录限流。

确认配置后启动：

```bash
docker compose pull
docker compose up -d
docker compose logs -f app
```

默认访问地址为 `http://服务器IP:3188`。SQLite 和登录同步图片统一保存在当前目录的 `./data`：

```text
mewimage/
├── docker-compose.yml
├── .env
└── data/
    ├── mew-image.db
    └── assets/
```

首次打开页面后，注册第一个账号，并在折叠的管理员初始化入口填写 `MEW_ADMIN_TOKEN`。第一个管理员建立后，普通用户注册会进入待审批状态。

说明：

- `docker-compose.yml` 本身不需要修改；镜像、端口和应用配置全部从同目录 `.env` 读取。
- 默认拉取 `mewlab/mewimage:latest`，可以通过 `.env` 中的 `MEW_DOCKER_IMAGE` 固定具体版本。
- 当前 Docker Hub 镜像提供 `linux/amd64` 架构；ARM64 服务器或 NAS 需要后续发布多架构镜像。
- 默认映射宿主机端口 `3188`，可以通过 `MEW_HOST_PORT` 修改。
- 默认启用 Local 图片存储，不需要额外部署 S3，登录同步图片会写入 `./data/assets`。
- 默认设置 `1 GiB` 容器内存上限，可以通过 `MEW_MEMORY_LIMIT` 调整。


## 本地开发

1. 安装依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
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


### 账号规则

- 普通用户注册后状态为 `pending`，可以登录查看状态和修改密码，但不能使用云端同步或服务器资源存储。
- 管理员在设置菜单的“用户管理”里批准用户后，用户状态变为 `approved`。
- 管理员可以禁用或恢复用户；后端会阻止管理员禁用当前登录的自己，避免误锁。
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
