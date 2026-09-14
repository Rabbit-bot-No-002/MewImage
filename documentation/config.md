# 环境变量参考

> .env 里每个变量的默认值、作用与改动风险，按用途分组。

本页由仓库的 `.env.example` 直接生成，分组与注释一一对应。**唯一事实来源是 `.env.example` 本身**；升级镜像后建议重新对照一次。

> 💡 **改完 .env 必须重建容器**
>
> 环境变量在容器创建时固定。`docker compose up -d` 会检测变化并自动重建；`docker compose restart` 不会重读 `.env`。

## Docker Compose 基础设置

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_DOCKER_IMAGE`<br>`MEW_CONTAINER_NAME` | `mewlab/mewimage:latest`<br>`mew-image` | Docker Hub 镜像。需要固定版本时可改成 mewlab/mewimage:1.1.1 |
| `MEW_HOST_PORT`<br>`MEW_MEMORY_LIMIT` | `3188`<br>`1g` | 该端口会映射到容器内部固定的 3000 端口。 |
| `MEW_HOST_BIND` | `127.0.0.1` | Compose 默认只监听本机，配合 Nginx/NPM/Caddy 使用更安全。<br>需要直接通过服务器 IP 访问时显式改为 0.0.0.0。<br>容器化 NPM 不能直接访问宿主机 127.0.0.1，应优先使用共享 Docker 网络。 |

## 首次部署必须修改

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_AUTH_SECRET` | *(空)* | 用于设备/IP 摘要和认证保护，必须使用固定强随机值。<br>生成命令：openssl rand -hex 32 |
| `MEW_MANAGED_PROVIDER_SECRET` | *(空)* | 托管账号服务商配置的服务端加密密钥。生成：openssl rand -hex 32<br>启用托管账号前必须设置并固定备份；丢失后已有托管配置无法恢复。<br>兼容旧变量名 MEW_IMAGE_MANAGED_PROVIDER_SECRET，优先读取下方新名称。 |
| `MEW_ADMIN_TOKEN` | *(空)* | 首个管理员初始化口令。首次注册管理员时填写，建议使用密码管理器生成。<br>生成命令：openssl rand -base64 32 |
| `MEW_ALLOWED_ORIGINS` | `http://127.0.0.1:3188,http://localhost:3188,http://127.0.0.1:3000,http://localhost:3000,http://127.0.0.1:8080,http://localhost:8080` | 允许访问后端的网页来源，多个地址使用英文逗号分隔，末尾不要加 /。<br>公网 HTTPS 示例：https://img.example.com<br>直接通过服务器端口访问示例：http://你的服务器IP:3188<br>下方默认值同时兼容 Docker 本机访问和前端开发端口；正式上线应只保留真实站点地址。 |
| `MEW_PUBLIC_BASE_URL` | *(空)* | 对外分享使用的公开站点来源，用于生成绝对 Open Graph 图片地址和 canonical URL。<br>只填写协议、域名和可选端口，不要包含路径或末尾斜杠；本地开发可留空。 |

## HTTPS 与反向代理

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_SESSION_SECURE` | `false` | 正式 HTTPS 域名部署改为 true；直接使用 HTTP/IP 测试时保持 false。 |
| `MEW_TRUST_PROXY_HEADERS` | `false` | 只有后端无法绕过 Nginx/NPM 直接从公网访问时才能设为 true。 |
| `MEW_TRUSTED_PROXY_CIDRS` | *(空)* | 开启代理头信任时必须填写实际反向代理的 IP/CIDR；不匹配的连接仍使用直连 IP。<br>Docker 同网络反代常见示例：172.16.0.0/12；请按实际网络缩小范围。 |
| `MEW_ALLOW_ADMIN_SETUP` | `true` | 是否允许通过初始化口令创建第一个管理员。 |

## 注册、登录与密码哈希限制

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_REGISTER_DEVICE_LIMIT` | `3` | 同一浏览器设备最多成功注册账号数；0 表示关闭。 |
| `MEW_REGISTER_IP_LIMIT`<br>`MEW_REGISTER_WINDOW_SECONDS` | `10`<br>`86400` | 同一 IP 每 86400 秒最多尝试注册 10 次。 |
| `MEW_LOGIN_IP_LIMIT`<br>`MEW_LOGIN_WINDOW_SECONDS` | `20`<br>`600` | 同一 IP 每 600 秒最多尝试登录 20 次。 |
| `MEW_LOGIN_FAILURE_LIMIT`<br>`MEW_LOGIN_LOCK_SECONDS` | `5`<br>`300` | 同一账号连续输错 5 次密码后锁定 300 秒。 |
| `MEW_AUTH_HASH_CONCURRENCY` | `2` | 同时运行的 Argon2 密码哈希/校验任务数，小型服务器建议保持 2。 |

## 游客代理与第三方服务商

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_GUEST_PROXY` | `true` | 游客可使用瞬时生成代理，但工作区和图片不会写入服务器存储。 |
| `MEW_GUEST_GENERATION_MAX_ACTIVE`<br>`MEW_GUEST_IMAGE_FETCH_MAX_ACTIVE` | `4`<br>`2` | 同一游客 IP 的活动任务与图片代理并发上限。并发上限始终生效。<br>旧版 *_CONCURRENCY 名称仍可读取，新部署统一使用下列名称。 |
| `MEW_GUEST_GENERATION_RATE_LIMIT`<br>`MEW_GUEST_IMAGE_FETCH_RATE_LIMIT`<br>`MEW_GUEST_RATE_WINDOW_SECONDS` | `30`<br>`120`<br>`600` | 同一游客 IP 在统计窗口内的调用上限；设为 0 可关闭频率限制。<br>旧版 MEW_GUEST_IMAGE_RATE_LIMIT 仍可读取。 |
| `MEW_ENFORCE_HOST_WHITELIST`<br>`MEW_TRUSTED_HOSTS` | `false`<br>*(空)* | 默认允许公网第三方中转站，同时拒绝本机、私网和链路本地地址。<br>若只允许固定上游，可设为 true，并填写 MEW_TRUSTED_HOSTS。 |
| `MEW_ALLOW_HTTP_UPSTREAM` | `false` | 上游默认必须使用 HTTPS。只应在完全了解明文传输风险时开启 HTTP。 |
| `MEW_ALLOW_LOOPBACK_UPSTREAM` | `false` | 仅供本机开发：允许显式访问 127.0.0.1、::1 或 localhost。<br>只有后端自身也监听环回地址时才生效；不会放开 10.x/172.16-31.x/192.168.x。<br>明文环回上游还必须同时开启 MEW_ALLOW_HTTP_UPSTREAM。生产部署请保持 false。 |
| `MEW_DEV_BYPASS_UPSTREAM_SSRF` | `false` | 仅供本机开发：跳过生成、图片下载（含重定向）的主机与解析 IP 安全检查。<br>可用于局域网上游或代理软件 Fake-IP DNS；仅在后端监听环回地址时生效。<br>HTTP 仍需 MEW_ALLOW_HTTP_UPSTREAM=true；生产部署请保持 false。 |
| `MEW_PROXY_MEMORY_BUDGET_MIB` | *(空)* | 留空时根据容器/主机内存自动计算，1 GiB 容器通常得到约 409 MiB。 |

## 登录用户图片存储

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MEW_ASSET_STORE` | `local` | local：保存到宿主机 ./data/assets，个人部署推荐。<br>s3：使用下方 S3 兼容对象存储。<br>disabled：禁用云端图片存储，游客本地模式仍可使用。 |
| `MEW_MAX_ASSET_MIB`<br>`MEW_USER_ASSET_QUOTA_MIB` | `64`<br>`5120` | 单个云端图片与每个已审批用户的默认存储上限；用户超额后仍可读取和删除。 |
| `MEW_GALLERY_ASSET_QUOTA_MIB`<br>`MEW_GALLERY_ASSET_QUOTA_COUNT` | `5120`<br>`20000` | 模板广场使用独立的公共资源命名空间和总配额（包含派生缩略图）；达到上限后管理员仍可删除旧模板。 |
| `MEW_S3_BUCKET`<br>`MEW_S3_REGION`<br>`MEW_S3_ENDPOINT`<br>`MEW_S3_ACCESS_KEY`<br>`MEW_S3_SECRET_KEY` | *(空)*<br>`us-east-1`<br>*(空)*<br>*(空)*<br>*(空)* | 只有 MEW_ASSET_STORE=s3 时才需要填写。 |
---

[← 文档目录](README.md) · [上一页：部署](deploy.md) · [下一页：工作台与生图](workbench.md)
