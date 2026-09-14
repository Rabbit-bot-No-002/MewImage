# 架构与设计原则

> 项目定位、五条设计原则、三个 crate 的职责、数据分层与前后端契约。

喵图 MewImage 是一个用 Rust 构建的本地优先图片生成 Web 应用。浏览器里的工作台是主入口，后端提供同源生成代理、账号权限、手动同步和图片远程存储。

## 项目定位

- 不登录也能完成主要生成工作：历史、会话、参考图、收藏、服务商配置和 API Key 默认只存在浏览器本地。
- 登录本身不带来新能力；再通过管理员审批之后，才获得跨设备同步和服务器图片存储。
- 同步是手动触发的增强能力，不在登录、关闭菜单或保存设置时自动执行。
- 后端承担同源生成代理、账号与权限管理、手动同步、图片远程存储和第三方服务商请求编排。
- 部署面向单人和小规模共享，本地文件存储与 S3 兼容对象存储二选一。

## 五条核心设计原则

| 原则 | 具体含义 |
| --- | --- |
| 本地优先 | `IndexedDB` 是游客与日常工作台的主数据源。页面交互先更新内存状态，再在浏览器空闲时批量落盘；原图与主工作区快照分离，避免每次操作都序列化大量 Base64。未登录时不会把历史、图片、会话、收藏或服务商配置写进服务器数据库和对象存储。 |
| 同步增强而非同步依赖 | 登录用户仍以本地工作区为主。只有点击「立即同步」才执行上传、拉取与合并，同步失败也不会破坏当前本地工作区。服务器是跨设备同步节点，不是输入、切会话和看图的前置依赖。 |
| 协议隔离三分法 | 用户可见的服务商类型固定为 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容` 三类，不混用请求协议。`CustomHttp` 保留历史兼容与扩展能力，只对已审批登录用户开放，不作为普通用户主入口。 |
| 安全边界 | 代理不访问本机、私网、链路本地等危险目标；用户、资源、同步快照和对象键按用户隔离；管理员能力一律由后端鉴权，前端隐藏按钮不算权限；危险删除统一走项目内的二次确认浮层。 |
| 性能优先 | 输入、删字、切会话、开关菜单和详情页不触发整包同步或同步写盘；画廊、收藏夹和原图分页或懒加载；后端不长期缓存已生成图片的完整 Base64；大型生成请求结束后尽快释放临时内存。 |

## 技术栈

Rust 2024 Edition workspace，三个 crate：

```text
backend/   Axum 后端：认证、同步、代理与资源存储
frontend/  Leptos CSR WebAssembly 前端
shared/    前后端共享类型、协议、解析器、尺寸与同步算法
```

前端用 `Leptos 0.8` CSR 编译到 `WebAssembly`，由 `Trunk` 构建静态资源，靠 `gloo-net`、`gloo-file`、`gloo-timers` 与 `web-sys`、`js-sys`、`wasm-bindgen` 操作浏览器，`Rexie` 封装 `IndexedDB`。API Key 在浏览器端用 `AES-256-GCM-SIV` 加密，可信设备同步密钥由 `PBKDF2-HMAC-SHA256` 派生，ZIP 导入导出用 `zip` 在浏览器内完成。

后端用 `Axum 0.8` 与 `Tokio`，上游请求走 `Reqwest + rustls`，数据库是 `SQLx + SQLite`，会话用 `tower-sessions` 的 SQLite store，密码用 `Argon2`，对象存储走 AWS SDK for Rust，日志用 `tracing`。Linux GNU 环境下，大型请求结束后会执行受控 `malloc_trim`，把堆内存还给系统。

部署形态是多阶段 `Dockerfile`：构建阶段用 Rust 与 Trunk，运行阶段基于 `debian:bookworm-slim`，单容器同时提供 API 与前端静态文件，持久卷固定为 `/data`。

## 总体架构

```text
浏览器 Leptos CSR
├── 内存响应式状态
├── IndexedDB 工作区元数据 / 图片 payload
├── localStorage 可信设备密钥与同步开关
├── 服务商直连请求
└── 同源 multipart 代理请求
        ↓
Axum 后端
├── 服务商协议编排与响应解析
├── 登录、审批和管理员权限
├── SQLite 会话与同步元数据
├── Local/S3 图片资源存储
└── 前端静态文件托管
```

完整部署通过 Axum 地址访问页面，前端与 API 保持同源。只部署静态文件时页面能打开，但代理、账号、同步和远程资源能力都不完整，所以纯静态部署不是推荐形态。

## 本地数据存储分层

`IndexedDB` 数据库名为 `mew-image-local`：

| Store | 存什么 |
| --- | --- |
| `kv` | 工作区快照、配置和偏好 |
| `asset_blobs` | v3 图片 Blob payload，新数据只写这里 |
| `asset_payloads` | v2 旧版 Data URL payload，只读兼容并按需迁移 |

工作区快照包含服务商配置元数据、生成任务、会话、图片资源引用与元数据、收藏状态与收藏文件夹、UI 偏好、同步 checkpoint 和同步删除墓碑。

`localStorage` 只放设备级轻量数据：API Key 同步开关，以及按用户隔离的可信设备同步密钥。

图片按 `SHA-256` 建立去重依据，任务只引用资源，同一张参考图不会在每个任务里重复保存二进制。元数据记录真实 MIME、字节数、真实宽高、来源任务和远程对象键；缩略图最长边约 320 像素，用于画廊快速加载；原图按需加载并以可回收 Blob URL 显示。原图内存缓存是 LRU，默认最多 6 张、约 48 MiB，超限时只清掉内存里的 `data_url`，不动 `IndexedDB` 里的原文件。

## 后端数据库

| 表 | 职责 |
| --- | --- |
| `users` | 账号、密码哈希、角色和审批状态 |
| `registration_devices` | 设备注册计数 |
| `auth_rate_limits` | 注册、登录和账号锁定数据 |
| `sync_snapshots` | 用户同步快照 |
| `provider_templates` | 用户自定义服务商模板，主键为 `(user_id, id)` |
| `assets` | 服务器图片索引 |
| `upload_tokens` | 短期一次性上传凭证 |
| `mew_image_sessions` | `tower-sessions` 持久化登录会话 |

广场、托管账号和审计另有一组表：`gallery_templates`、`gallery_tags`、`gallery_template_tags`、`gallery_assets`、`gallery_template_assets`、`gallery_likes`、`gallery_staged_objects`、`managed_provider_configs`、`managed_provider_templates` 和 `admin_audit_logs`。

SQLite 启用 WAL、`synchronous=NORMAL` 和 busy timeout；同步拉取后的合并与快照写回在同一个事务里完成。游客生成不写入同步表和资产表。

## 前后端契约

`shared` 是唯一的类型来源：服务商类型与访问模式、生成请求与结果、代理任务状态与响应、参数快照、会话与任务记录、图片资源引用、外观偏好、同步信封与墓碑，还有各服务商响应的解析函数以及尺寸与宽高比算法。同步 schema 版本常量 `SYNC_SCHEMA_VERSION` 也在这里，当前是 `3`。前端与后端共用同一份校验与归一化逻辑，避免两边对参数的理解分叉。

## 前端可维护性结构

`frontend/src/main.rs` 只保留模块声明、WASM 启动函数和 `App` 挂载，业务按职责拆到 `frontend/src/app/`：

| 模块 | 职责 |
| --- | --- |
| `state.rs` | 工作区、生成编辑器、账号、界面、持久化五组领域状态与默认值 |
| `derived.rs` | `AppDerived`：当前配置、会话、参考图、画廊、收藏夹和分页等 Memo 派生状态 |
| `effects.rs` | 主题、键盘、浮动提示、`IndexedDB` 恢复、认证初始化、草稿同步和分页校正等根级 Effect |
| `actions/` | 账号、数据、生成、工作区和预览动作 |
| `components/` | 顶栏、设置、收藏夹、画廊、工作区、预览和全局浮层 |
| `utils/` | 分辨率、图片处理、工作区算法、格式化、同步、音频和持久化纯函数 |

`AppDerived` 通过 Leptos Context 提供，组件用 `expect_context::<AppDerived>()` 取用，不必逐层传参；派生状态用 `Memo` 缓存，默认值集中创建，不复制任务和图片等大型集合。这层重构不改后端接口、共享数据结构、`IndexedDB` 格式、同步协议和备份 Schema。

## 相关页面

- [安全边界](security.md)：代理、上传、会话与托管账号的具体约束
- [后端 API](api.md)：路由清单、能力协商与错误语义
- [手动云同步](sync.md)：合并、墓碑与图片同步的实际行为
---

[← 文档目录](README.md) · [上一页：本地开发与联调](local-dev.md) · [下一页：安全边界](security.md)
