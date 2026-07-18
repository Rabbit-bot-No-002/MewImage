# MewImage

一个使用 Rust 构建的前后端结合图片生成平台。

- 前端：`Leptos CSR`
- 后端：`Axum + SQLite`
- 资源存储：本地文件存储或 `S3 兼容对象存储`
- 模式：游客本地优先，登录后手动跨设备同步
- 代理：默认是“游客本地 + 受限代理模式”，允许公网图像上游，始终拒绝本机/内网等危险目标

## 当前实现

- 历史、会话、收藏、参考图、界面偏好默认保存在浏览器 IndexedDB
- 登录只是同步增强能力，不是使用前提
- 注册用户默认需要管理员审批，审批通过后才能使用云端同步和服务器资源存储
- 注册与登录带设备/IP 限流；账号连续输错 5 次密码后默认锁定 5 分钟
- 用户界面会显示当前账号保存在服务器的图片数量；管理员列表会显示每个用户的服务器图片数量
- 支持 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容`
- `OpenAI Image` 的 Responses API 使用流式图像工具响应，兼容 completed、output item、partial image 事件和常见网关包装结构
- 后端会对代理上游做白名单校验，拒绝内网、本机和未授权第三方域名
- 登录态远程资源上传可使用本地文件存储；需要云对象存储时可切换到 S3 兼容模式
- 设置面板使用侧边栏分页，按服务商、账号、密码、用户管理、数据管理和关于分区显示
- 数据管理支持本地 ZIP 导出、合并导入、分类清除，以及登录用户自助查看和清除自己的云端数据
- 同步协议已升级到 v2：删除会话、任务、图片或配置会生成同步墓碑，其他设备同步后会执行同样删除，不再从云端恢复旧记录
- 网站标签页使用兼容 ICO 图标，页面品牌区和关于页使用高清 SVG 图标

### 多设备删除同步

- 单项删除会写入带时间戳的墓碑记录，并随手动同步上传。
- 合并时记录与墓碑按更新时间竞争；时间相同时删除优先，旧设备无法重新上传已删除记录。
- 其他设备同步后会删除对应 IndexedDB 图片 payload，并清理失效的参考图、连续修改和预览状态。
- 服务器确认图片墓碑后会删除对应 SQLite 资产行；仅当对象键没有其他资产引用时，才删除本地文件或 S3 对象。
- 墓碑当前长期保留，确保长时间离线的设备重新上线后也不会让旧数据复活。
- “清除本地数据”仍然只影响当前浏览器，不创建批量云端删除墓碑；需要删除云端数据时请使用数据管理中的云端清除功能。
- v1 同步快照可以继续读取，首次由新版前后端完成同步后会升级为 v2。

## 数据管理

设置菜单中的“数据管理”分为“本地数据”和“云端数据”，两侧操作互不联动：

- 本地导出在浏览器内生成 ZIP，不会把备份上传到服务器。
- ZIP 包含版本化 `manifest.json` 和独立的 `assets/` 图片文件；相同图片文件按 SHA-256 去重保存。
- 明文 API Key 和上游响应中的大段 Base64 不会写入备份；已加密配置可以随工作区元数据保留。
- 本地导入默认采用合并模式：稳定 ID 按更新时间取新值，上传参考图按 SHA-256 复用，当前浏览器中的明文 Key 优先保留。
- 导入会限制 ZIP 条目数、路径和解压总大小，并校验图片长度与 SHA-256；损坏或危险备份不会写入工作区。
- 本地清除可分别清除历史与图片、服务商配置、界面偏好或全部本地数据，不会删除云端备份。
- 云端页仅对已登录且审批通过的用户开放，可查看图片数量、占用空间、模板和同步快照状态。
- 云端清除只作用于当前账号，可分别删除同步数据与图片、服务商模板或全部云端数据；账号和浏览器本地工作区会保留。

有图片正在生成时，本地导出、导入和清除会被阻止，避免异步生成结果与数据操作互相覆盖。云端统计和清除接口分别为 `GET /api/data/stats` 与 `POST /api/data/clear`，均要求审批通过的登录会话。

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

当前推荐使用较短的 `MEW_*` 变量名。旧版 `MEW_IMAGE_*` 变量仍可继续使用；如果新旧变量同时存在，后端优先读取新变量，方便现有部署平滑迁移。

常用变量迁移对照：

| 新变量 | 旧变量 |
| --- | --- |
| `MEW_LISTEN` | `MEW_IMAGE_LISTEN` |
| `MEW_DATABASE_URL` | `MEW_IMAGE_DATABASE_URL` |
| `MEW_FRONTEND_DIST` | `MEW_IMAGE_FRONTEND_DIST` |
| `MEW_SESSION_SECURE` | `MEW_IMAGE_SESSION_SECURE` |
| `MEW_AUTH_SECRET` | `MEW_IMAGE_AUTH_SECRET` |
| `MEW_TRUST_PROXY_HEADERS` | `MEW_IMAGE_TRUST_PROXY_HEADERS` |
| `MEW_ADMIN_TOKEN` | `MEW_IMAGE_ADMIN_SETUP_TOKEN` |
| `MEW_ALLOWED_ORIGINS` | `MEW_IMAGE_ALLOWED_WEB_ORIGINS` |
| `MEW_ASSET_STORE` | `MEW_IMAGE_ASSET_STORE` |
| `MEW_LOCAL_ASSET_DIR` | `MEW_IMAGE_LOCAL_ASSET_DIR` |

其余短变量及推荐值以 [.env.example](.env.example) 为准。

### 基础项

- `MEW_LISTEN`
  - 后端监听地址。
  - 本地推荐：`127.0.0.1:3000`
  - 容器/服务器推荐：`0.0.0.0:3000`

- `MEW_DATABASE_URL`
  - SQLite 连接串。
  - 本地推荐：`sqlite://./data/mew-image.db?mode=rwc`
  - 容器推荐：`sqlite:///data/mew-image.db`

- `MEW_FRONTEND_DIST`
  - 后端托管的前端静态目录。
  - 当前推荐：`./frontend/dist-app`
  - Docker 中推荐：`/app/frontend/dist-app`

- `MEW_SESSION_SECURE`
  - 是否只在 HTTPS 下发送会话 Cookie。
  - 本地开发：`false`
  - 正式 HTTPS 站点：`true`

### 认证防护

- `MEW_AUTH_SECRET`
  - 用于对设备标识和客户端 IP 做不可逆摘要，生产环境必须固定配置。
  - 推荐执行 `openssl rand -hex 32` 生成，不要与 API Key、账号密码共用。
  - 未配置时会回退使用管理员初始化口令；两者都没有时每次启动会生成临时值，设备注册统计在重启后将无法继续匹配旧摘要。

- `MEW_TRUST_PROXY_HEADERS`
  - 是否信任 `X-Forwarded-For` 和 `X-Real-IP`。
  - 直接暴露后端端口时必须为 `false`，否则客户端可以伪造来源 IP 绕过限流。
  - 后端只能通过 Nginx Proxy Manager 等可信反向代理访问时设为 `true`。

- `MEW_REGISTER_DEVICE_LIMIT`
  - 同一浏览器设备最多成功注册账号数，默认 `3`。
  - 设备通过长期 HttpOnly Cookie 识别；清除浏览器 Cookie 可以更换设备标识，因此还必须配合 IP 限流和管理员审批。
  - 设置为 `0` 可关闭设备数量限制。

- `MEW_REGISTER_IP_LIMIT` / `MEW_REGISTER_WINDOW_SECONDS`
  - 默认同一 IP 每 `86400` 秒最多尝试注册 `10` 次。

- `MEW_LOGIN_IP_LIMIT` / `MEW_LOGIN_WINDOW_SECONDS`
  - 默认同一 IP 每 `600` 秒最多尝试登录 `20` 次。

- `MEW_LOGIN_FAILURE_LIMIT` / `MEW_LOGIN_LOCK_SECONDS`
  - 默认同一账号连续输错 `5` 次密码后锁定 `300` 秒，登录成功或修改密码成功后清零。
  - `MEW_LOGIN_FAILURE_LIMIT=0` 可关闭账号锁定，但不建议在公开站点使用。

- `MEW_AUTH_HASH_CONCURRENCY`
  - Argon2 密码哈希和校验的最大并发数，默认 `2`。
  - 小型云服务器建议保持 `2`；CPU 核心较多时可谨慎提高。

### 账号与审批

- `MEW_ADMIN_TOKEN`
  - 首个管理员初始化口令。
  - 部署时请改成强随机字符串，不要使用示例值。
  - 当系统里还没有管理员时，注册页面填写该口令且密码符合强度要求，即可创建首个 `admin` 账号。

- `MEW_ALLOW_ADMIN_SETUP`
  - 是否允许通过初始化口令创建首个管理员。
  - 推荐：`true`
  - 首个管理员创建后，即使保持 `true`，后续注册也不会自动成为管理员。

账号规则：

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

旧账号升级管理员：

- 如果升级前已存在账号、但数据库中还没有任何管理员，先登录该旧账号。
- 在“账号与同步”中展开“使用管理员初始化口令”，输入 `MEW_ADMIN_TOKEN` 后可将当前账号安全升级为首个管理员。
- 如果系统已经存在管理员，该入口不会显示，后端也会拒绝升级请求。

### 安全与代理

- `MEW_ALLOWED_ORIGINS`
  - 允许带 Cookie 调用后端的前端来源，多个值用英文逗号分隔。
  - 本地推荐：
    `http://127.0.0.1:3000,http://localhost:3000,http://127.0.0.1:8080,http://localhost:8080`
  - 生产推荐只保留你自己的正式前端域名。

- `MEW_GUEST_PROXY`
  - 是否允许游客使用后端代理生成。
  - 推荐：`true`
  - 若你只想开放登录用户代理，可改为 `false`

- `MEW_CUSTOM_PROVIDER_LOGIN`
  - 是否要求自定义服务商必须登录后才能使用。
  - 推荐：`true`

- `MEW_ENFORCE_HOST_WHITELIST`
  - 是否强制代理上游必须命中白名单。
  - 默认推荐：`false`
  - 如果你的部署只允许固定服务商或固定中转站，可改为 `true`

- `MEW_TRUSTED_HOSTS`
  - 可选的第三方上游域名白名单，多个值用英文逗号分隔。
  - 开启 `MEW_ENFORCE_HOST_WHITELIST=true` 后，第三方中转站必须命中这里。
  - 示例：
    `openai.example.com,proxy.example.net`
  - 无论是否开启强制白名单，后端都会拒绝本机、私网 IP、链路本地地址等危险目标。

### 对象存储

- `MEW_ASSET_STORE`
  - 资源存储后端。
  - 推荐个人 Docker 部署：`local`
  - 可选：`s3`、`disabled`

- `MEW_LOCAL_ASSET_DIR`
  - `MEW_ASSET_STORE=local` 时的图片文件保存目录。
  - 本地推荐：`./data/assets`
  - Docker 推荐：`/data/assets`

- `MEW_S3_BUCKET`
- `MEW_S3_REGION`
- `MEW_S3_ENDPOINT`
- `MEW_S3_ACCESS_KEY`
- `MEW_S3_SECRET_KEY`

说明：

- 默认推荐使用 `local`，不需要额外部署 MinIO/S3，SQLite 和图片都可以放在 `/data` 持久卷内。
- 只有当 `MEW_ASSET_STORE=s3` 时，才需要配置 S3 兼容对象存储变量。
- `MEW_ASSET_STORE=disabled` 时，游客本地生成和本地历史仍可正常使用，但登录态远程资源上传、跨设备同步图片引用等能力会受限。

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

### 数据规模与性能

数十个会话、数百张图片通常不会让容器因为“已存图片数量”直接耗尽内存：服务器只在请求时读取图片文件，SQLite 保存的是索引和同步快照，不会在启动时把所有原图加载进内存。前端结果画廊每页渲染 10 张，收藏夹每页渲染 9 张，并使用约 320 像素缩略图；原图主体独立保存在 IndexedDB，需要查看详情、继续修改或同步时才读取。

当前性能策略：

- 手动同步会逐张读取和上传尚未进入服务器存储的图片，成功一张就立即保存远程对象键；中途中断后，下次只继续剩余图片。最终同步请求只包含轻量元数据，不再一次携带全部 Base64 原图。
- 浏览器原图内存缓存采用 LRU，默认最多保留 6 张或约 48 MiB 原始图片数据；正在生成或预览所需的图片会临时受保护，IndexedDB 中的原文件不会被淘汰。
- 普通结果画廊每页 10 张，收藏夹每页 9 张；收藏夹桌面布局为每行 3 张、共 3 行，并复用结果画廊的页码轮交互。
- 后端代理生成最多允许 5 个并发任务，容器示例设置 1 GiB 内存上限。多张 4K Base64 图片同时生成时仍可能出现较高瞬时内存，极端情况下容器会被内存上限重启，但已落盘数据不会因此丢失。
- 切换到 S3 主要节省服务器本地硬盘，不会显著降低生成代理和 Base64 解析期间的瞬时内存。

大量图片首次同步仍会受网络带宽影响，但内存峰值按单张图片控制。若同时生成多张 4K 图片，建议根据服务器内存适当降低上游并发或继续保留容器内存上限。

## 推荐部署策略

### 1. 纯本地优先 + 受限代理

适合个人使用或轻量分享。

- 开启 `MEW_GUEST_PROXY=true`
- 保持 `MEW_ENFORCE_HOST_WHITELIST=false`，方便使用常见 API 中转站
- 如果是公开多人服务，再考虑把固定中转站写进 `MEW_TRUSTED_HOSTS` 并开启强制白名单
- 如果需要登录同步图片，推荐先使用 `MEW_ASSET_STORE=local`

效果：

- 游客数据不写入云端数据库和对象存储
- 服务端只承担临时请求转发、会话和可选同步职责

### 2. 正式 HTTPS 部署

推荐：

- `MEW_SESSION_SECURE=true`
- `MEW_AUTH_SECRET` 使用 `openssl rand -hex 32` 生成的独立随机值
- `MEW_ALLOWED_ORIGINS` 只写正式域名
- `MEW_ADMIN_TOKEN` 使用强随机字符串，例如密码管理器生成的 32 位以上随机值
- 为登录同步功能配置 SQLite 持久卷，并选择 `local` 或 S3 兼容对象存储

首次上线流程：

1. 在 compose 或 `.env` 中设置 `MEW_ADMIN_TOKEN`。
2. 打开网页，注册第一个账号，填写用户名、强密码、确认密码和管理员初始化口令。
3. 第一个账号会成为管理员并自动审批通过。
4. 朋友自行注册后会进入待审批状态，由管理员在设置菜单中批准。
5. 审批通过前，朋友仍可使用游客/本地能力，但不能写入服务器资源存储或同步数据。

### Nginx Proxy Manager 获取真实 IP

如果 MewImage 位于 Nginx Proxy Manager 后面，在 NPM 对应 Proxy Host 的 `Advanced` 中确认包含：

```nginx
proxy_set_header X-Real-IP $remote_addr;
proxy_set_header X-Forwarded-For $remote_addr;
proxy_set_header X-Forwarded-Proto $scheme;
```

然后在 MewImage 容器中设置：

```yaml
environment:
  MEW_TRUST_PROXY_HEADERS: "true"
  MEW_AUTH_SECRET: "使用 openssl rand -hex 32 生成的随机值"
```

安全要求：

- MewImage 后端端口不能绕过 NPM 直接暴露给公网，否则攻击者可以自行构造 `X-Forwarded-For`。
- 这里故意使用 `$remote_addr` 覆盖 `X-Forwarded-For`，不要改成保留客户端输入的 `$proxy_add_x_forwarded_for`；MewImage 会优先读取 `X-Real-IP`。
- NPM 和 MewImage 在同一 Docker 网络时，推荐只使用容器名和内部端口转发，不发布 MewImage 的宿主机公网端口。
- 如果必须映射宿主机端口，应通过防火墙限制来源，只允许 NPM 所在主机或网络访问。
- NPM 通常已经自动设置上述请求头；这里的配置用于明确保证其存在。可在浏览器登录测试后查看限流日志或临时错误响应确认真实 IP 是否生效。

## 生产构建

```bash
cargo build -p mew-image-backend --release
cd frontend
trunk build --release --dist dist-app
```

然后让后端通过 `MEW_FRONTEND_DIST` 指向 `frontend/dist-app`。

## Docker

```bash
docker compose up --build
```

当前仓库里的 Docker 示例包含：

- `app`：后端 + 已构建前端静态文件

说明：

- 应用默认监听 `3000`
- `docker-compose.yml` 默认使用 `MEW_ASSET_STORE=local`
- 持久数据默认挂载到宿主机 `./data`
- 容器默认设置 `1GB` 内存上限，代理生成超过 5 个并发时会在后端排队
- SQLite 在容器内路径为 `/data/mew-image.db`
- 同步图片在容器内路径为 `/data/assets`
- 本地与 S3 对象键统一为 `users/{user_id}/assets/...`，不同用户资源按目录隔离
- 首次部署请替换 `MEW_ADMIN_TOKEN`，然后用该口令注册首个管理员
- 若要正式上线，请收紧 `MEW_ALLOWED_ORIGINS`

## 安全说明

- 通用开放代理已移除，不再支持任意 URL 中转
- 后端会拒绝 `localhost`、私网 IP、链路本地地址等危险目标
- 第三方公网中转站默认允许；部署者可通过 `MEW_ENFORCE_HOST_WHITELIST=true` 改成严格白名单模式
- 上游只返回图片 URL 时，后端会尝试下载公网图片并回填到本地结果；该下载同样拒绝本机、私网和链路本地地址
- 自定义服务商默认需要登录，且仍受上游白名单约束
- 会话已落 SQLite 持久化，重启后不会像内存会话那样全部失效

## 验证状态

当前已完成：

- `cargo check`
- `cargo test --all --no-run`

说明：

- 仓库内已提供 Docker 产物，但当前环境未实际执行容器实跑验证
- 如需发布前更稳妥验证，建议再运行后端集成测试并在目标 Docker 环境实跑
