# 安全边界

> 代理密钥处理、上游 SSRF 防护、上传校验、会话与托管账号隔离。

喵图的安全模型建立在一个前提上：浏览器里的数据属于用户自己，服务器只在用户明确要求时才短暂接触凭据和图片。

## 安全总原则

- 代理不访问本机、私网、链路本地等危险目标。
- 用户、资源、同步快照和对象键按用户隔离，跨用户读取会被拒绝。
- 管理员能力只由后端鉴权，不依赖前端隐藏按钮；所有管理员和用户数据接口都会重新检查身份、状态和角色。
- 危险删除统一使用项目内的二次确认浮层。
- 后端日志不记录 `Authorization`、API Key、图片 Base64 和其他敏感请求字段。

## 游客代理的密钥处理

游客和已审批用户都能走同源代理。API Key 只在请求期间进入后端内存，不写入数据库、对象存储或业务日志；游客代理只做瞬时转发，不创建云端资产记录，也不生成同步快照，工作区和图片始终留在浏览器里。等待任务的大型参考图进入受限临时目录，凭据只留在内存，任务完成、取消、超时或重启后清理。

上游原始错误最多保留 64 KiB 返回给客户端诊断，并移除凭据、Cookie 和大段 Base64。

## 上游 SSRF 防护

生成统一走 `/api/providers/generate`，上游图片回填走 `/api/images/fetch`；通用任意 URL 转发接口已经移除。

无论是否开启域名白名单，以下目标始终被拒绝：`localhost`、回环地址、私网 IP、链路本地地址、非 HTTP/HTTPS scheme，以及解析后指向这些地址的目标。第三方公网中转站默认允许，因为这是实际使用的主要形态。

| 变量 | 默认值 | 作用 |
| --- | --- | --- |
| `MEW_ENFORCE_HOST_WHITELIST` | `false` | 设为 `true` 后只允许 `MEW_TRUSTED_HOSTS` 中的上游 |
| `MEW_TRUSTED_HOSTS` | *(空)* | 严格白名单模式下允许的上游域名列表 |
| `MEW_ALLOW_HTTP_UPSTREAM` | `false` | 上游默认必须用 HTTPS，只应在完全了解明文传输风险时开启 |
| `MEW_ALLOW_LOOPBACK_UPSTREAM` | `false` | 仅本机开发：允许显式访问 `127.0.0.1`、`::1` 或 `localhost` |
| `MEW_DEV_BYPASS_UPSTREAM_SSRF` | `false` | 仅本机开发：跳过生成与图片下载（含重定向）的主机和解析 IP 检查 |

> 🛑 **两个开发开关的生效前提**
>
> `MEW_ALLOW_LOOPBACK_UPSTREAM` 只允许显式环回目标，不会连带放开 `10.x`、`172.16-31.x`、`192.168.x`；`MEW_DEV_BYPASS_UPSTREAM_SSRF` 则会跳过生成、图片下载和重定向的主机与解析 IP 检查，适合局域网上游或代理软件的 Fake-IP DNS。两者都只在后端自身通过 `MEW_LISTEN` 监听环回地址时生效，监听 `0.0.0.0` 的容器和公网部署要保持 `false`。明文环回上游还需要同时开启 `MEW_ALLOW_HTTP_UPSTREAM`。开关没生效时后端启动日志会给出提示。

其余约束：

| 项目 | 规则 |
| --- | --- |
| 传输协议 | 上游默认只允许 HTTPS |
| DNS | 全部解析结果必须是公网地址，并在请求期间固定已验证地址以阻止 DNS 重绑定 |
| 重定向 | 生成请求不跟随重定向；图片下载最多手动跟随 3 跳，每跳重复安全校验 |
| 超时 | 上游连接 10 秒，图片下载 120 秒，生成任务 30 分钟 |
| 返回内容 | 图片代理只返回通过魔数校验的 PNG、JPEG 或 WebP，单图最大 64 MiB |
| `CustomHttp` | 固定在服务端仅向已审批登录用户开放，不能通过环境变量放宽 |

## 上传凭证与校验

图片上传初始化、字节上传和完成确认都要求已审批登录用户。对象存储未启用时，上传初始化返回明确错误。

| 环节 | 校验内容 |
| --- | --- |
| `POST /api/assets/upload-init` | 校验 `byte_len`、SHA-256 格式、MIME 与配额；签发 15 分钟后过期的 token 和固定对象键 |
| `PUT /api/assets/upload/{token}` | token 未过期且属于当前用户；`Content-Length` 与登记值一致；正文 SHA-256 与登记的 `sha256` 一致；魔数探测的 MIME 与声明一致 |
| `POST /api/assets/complete` | 复验实际大小、SHA-256 与图片魔数/MIME，然后在 SQLite 事务中登记正式资源 |

上传先写入 staging，完成接口核对通过后才登记正式资源，所以不会出现「SQLite 里有资产记录但对象不存在」的伪成功状态。token 完成后立即删除，禁止重复消费；关键路径会清理过期 token，过期凭证和孤儿对象由后台清理。单文件默认最大 64 MiB，每个已审批用户默认配额 5 GiB，超额后仍可读取、下载和删除已有资源。对象键统一为 `users/{user_id}/assets/...`，Local 路径拼接会拒绝 `..` 和绝对路径。

## 代理头信任与反向代理

> ⚠️ **`MEW_TRUST_PROXY_HEADERS` 只在后端无法绕过反代直连时开启**
>
> 开启后要用 `MEW_TRUSTED_PROXY_CIDRS` 限定真实反向代理的 IP/CIDR，不匹配的连接仍使用直连 IP。反向代理要覆盖 `X-Real-IP` 和 `X-Forwarded-For`；若把客户端自带的同名头原样透传，注册、登录限流和审计都会按伪造 IP 统计。

## CORS、Cookie 与会话

CORS 使用显式 origin 白名单（`MEW_ALLOWED_ORIGINS`），不镜像任意 Origin；带 Cookie 的认证接口只允许受信任来源。Session Cookie 使用 `SameSite=Lax`，HTTPS 部署通过 `MEW_SESSION_SECURE=true` 开启 Secure；会话存在 SQLite store 而非内存 store，过期会话每 15 分钟清理。登录成功时轮换 Session ID；改密、禁用、删除账号或角色与审批状态变化后，旧会话通过会话版本失效。响应统一带 CSP、`X-Content-Type-Options`、`Referrer-Policy`、`Permissions-Policy` 和防嵌入安全头，同时兼容 Blob 图片与 WASM 运行。

登录防护还包括限流与锁定：注册和登录按客户端 IP 设时间窗限制，同一账号连续输错 5 次密码后默认锁定 5 分钟；`Argon2` 哈希与校验由并发信号量限制，默认同时只跑 2 个，避免攻击打满 CPU。

## 托管账号的信息隔离

托管账号权限与已审批普通用户相同，但服务商连接信息由服务器托管，并强制通过代理使用。托管用户的浏览器、`IndexedDB`、同步快照、工作区 ZIP、任务详情和错误信息都拿不到上游地址、API Key、密文或 Key 提示。`POST /api/sync/push` 与 `GET /api/sync/pull` 对托管账号会清空 `configs` 字段，`POST /api/managed/providers/{config_id}/model` 只能切换管理员允许的当前模型；托管账号调用 `/api/providers/generate` 会被拒绝，必须走 `/api/managed/providers/generate`。

托管配置用 `MEW_MANAGED_PROVIDER_SECRET` 做带随机 Nonce 和 AAD 的认证加密；密钥丢失后已有托管配置无法恢复，启用前要固定备份。

## 管理接口、审计与内存预算

`/api/admin/*` 和 `/api/managed/*` 的每个接口都在后端校验管理员或托管账号身份，返回 `401` 而不是只靠前端隐藏入口。用户、托管配置和服务商模板的成功操作写入 `admin_audit_logs`，日志保留操作者、目标和批次快照，但不记录敏感连接信息。

内存预算同时是可用性保护，超限时返回错误而不是继续堆积：

| 项目 | 上限 |
| --- | --- |
| 活动生成任务 | 20 个 |
| `multipart` 请求体 | 192 MiB，最多 10 张参考图，单张最大 32 MiB |
| 字节预算 | 生成、原图上传下载和图片代理共享；下载响应持有预算直到正文发送结束 |

多张 4K Base64 图片仍会产生较高瞬时内存；容器内存上限是最终保险，不是常规内存目标。

## 相关页面

- [环境变量参考](config.md)：全部安全相关变量的默认值与改动风险
- [部署](deploy.md)：反代、Cookie 与容器权限的实际配置
- [托管账号](managed.md)：管理员侧的配置与分配流程
---

[← 文档目录](README.md) · [上一页：架构与设计原则](architecture.md) · [下一页：后端 API 参考](api.md)
