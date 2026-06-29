# MewImage

一个使用 Rust 构建的前后端结合图片生成平台。

- 前端：`Leptos CSR`
- 后端：`Axum + SQLite`
- 资源存储：`S3 兼容对象存储`
- 模式：游客本地优先，登录后手动跨设备同步
- 代理：默认是“游客本地 + 受限代理模式”，只允许访问受信任图像上游

## 当前实现

- 历史、会话、收藏、参考图、界面偏好默认保存在浏览器 IndexedDB
- 登录只是同步增强能力，不是使用前提
- 支持 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容`
- 后端会对代理上游做白名单校验，拒绝内网、本机和未授权第三方域名
- 登录态远程资源上传依赖对象存储；未配置对象存储时，游客本地生成仍可用

## 项目结构

```text
backend/   Axum 后端
frontend/  Leptos CSR 前端
shared/    前后端共享类型与同步/尺寸规则
```

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

## 环境变量

### 基础项

- `MEW_IMAGE_LISTEN`
  - 后端监听地址。
  - 本地推荐：`127.0.0.1:3000`
  - 容器/服务器推荐：`0.0.0.0:3000`

- `MEW_IMAGE_DATABASE_URL`
  - SQLite 连接串。
  - 本地推荐：`sqlite://./data/mew-image.db?mode=rwc`
  - 容器推荐：`sqlite:///data/mew-image.db`

- `MEW_IMAGE_FRONTEND_DIST`
  - 后端托管的前端静态目录。
  - 当前推荐：`./frontend/dist-app`
  - Docker 中推荐：`/app/frontend/dist-app`

- `MEW_IMAGE_SESSION_SECURE`
  - 是否只在 HTTPS 下发送会话 Cookie。
  - 本地开发：`false`
  - 正式 HTTPS 站点：`true`

### 安全与代理

- `MEW_IMAGE_ALLOWED_WEB_ORIGINS`
  - 允许带 Cookie 调用后端的前端来源，多个值用英文逗号分隔。
  - 本地推荐：
    `http://127.0.0.1:3000,http://localhost:3000,http://127.0.0.1:8080,http://localhost:8080`
  - 生产推荐只保留你自己的正式前端域名。

- `MEW_IMAGE_ENABLE_GUEST_PROXY`
  - 是否允许游客使用后端代理生成。
  - 推荐：`true`
  - 若你只想开放登录用户代理，可改为 `false`

- `MEW_IMAGE_REQUIRE_LOGIN_FOR_CUSTOM_PROVIDER`
  - 是否要求自定义服务商必须登录后才能使用。
  - 推荐：`true`

- `MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST`
  - 是否强制代理上游必须命中白名单。
  - 默认推荐：`false`
  - 如果你的部署只允许固定服务商或固定中转站，可改为 `true`

- `MEW_IMAGE_TRUSTED_PROVIDER_HOSTS`
  - 可选的第三方上游域名白名单，多个值用英文逗号分隔。
  - 开启 `MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST=true` 后，第三方中转站必须命中这里。
  - 示例：
    `openai.example.com,proxy.example.net`
  - 无论是否开启强制白名单，后端都会拒绝本机、私网 IP、链路本地地址等危险目标。

### 对象存储

- `MEW_IMAGE_S3_BUCKET`
- `MEW_IMAGE_S3_REGION`
- `MEW_IMAGE_S3_ENDPOINT`
- `MEW_IMAGE_S3_ACCESS_KEY`
- `MEW_IMAGE_S3_SECRET_KEY`

说明：

- 这些变量为空时，游客本地生成和本地历史仍可正常使用。
- 但登录态远程资源上传、跨设备同步图片引用等能力会受限。

## 推荐部署策略

### 1. 纯本地优先 + 受限代理

适合个人使用或轻量分享。

- 开启 `MEW_IMAGE_ENABLE_GUEST_PROXY=true`
- 保持 `MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST=false`，方便使用常见 API 中转站
- 如果是公开多人服务，再考虑把固定中转站写进 `MEW_IMAGE_TRUSTED_PROVIDER_HOSTS` 并开启强制白名单
- 如果暂时不需要跨设备同步图片，可先不配对象存储

效果：

- 游客数据不写入云端数据库和对象存储
- 服务端只承担临时请求转发、会话和可选同步职责

### 2. 正式 HTTPS 部署

推荐：

- `MEW_IMAGE_SESSION_SECURE=true`
- `MEW_IMAGE_ALLOWED_WEB_ORIGINS` 只写正式域名
- 为登录同步功能配置 SQLite 持久卷和 S3 兼容对象存储

## 生产构建

```bash
cargo build -p mew-image-backend --release
cd frontend
trunk build --release --dist dist-app
```

然后让后端通过 `MEW_IMAGE_FRONTEND_DIST` 指向 `frontend/dist-app`。

## Docker

```bash
docker compose up --build
```

当前仓库里的 Docker 示例包含：

- `app`：后端 + 已构建前端静态文件
- `minio`：本地对象存储示例

说明：

- 应用默认监听 `3000`
- MinIO 默认开放 `9000` 与控制台 `9001`
- 首次启动后请手动创建 `MEW_IMAGE_S3_BUCKET` 对应的桶
- `docker-compose.yml` 当前默认允许本机访问，若要正式上线，请收紧 `MEW_IMAGE_ALLOWED_WEB_ORIGINS`

## 安全说明

- 通用开放代理已移除，不再支持任意 URL 中转
- 后端会拒绝 `localhost`、私网 IP、链路本地地址等危险目标
- 第三方公网中转站默认允许；部署者可通过 `MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST=true` 改成严格白名单模式
- 上游只返回图片 URL 时，后端会尝试下载公网图片并回填到本地结果；该下载同样拒绝本机、私网和链路本地地址
- 自定义服务商默认需要登录，且仍受上游白名单约束
- 会话已落 SQLite 持久化，重启后不会像内存会话那样全部失效

## 验证状态

当前已完成：

- `cargo check`
- `cargo test --all --no-run`
- `cargo test -p mew-image-backend -- --nocapture`

说明：

- 仓库内已提供 Docker 产物，但当前环境未实际执行容器实跑验证
