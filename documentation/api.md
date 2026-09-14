# 后端 API 参考

> 按用途分组的路由清单、能力协商、同步语义与错误约定。

前端与后端同源部署，所有接口以 `/api` 开头，请求与响应默认是 JSON，生成代理走 `multipart`，资源上传走原始字节。本页路径取自 `backend/src/` 中的实际路由定义。

## 通用约定

- 认证靠 Session Cookie，登录成功时由后端设置；Cookie 为 `SameSite=Lax`，HTTPS 部署另加 Secure。
- 错误响应结构统一：

```json
{ "error": "人类可读的原因", "code": "可选机器码", "retry_after_seconds": 30 }
```

- 触发限流时另外返回 `Retry-After` 响应头。
- 请求体上限按用途分开：普通 JSON 2 MiB、同步 JSON 32 MiB、单资源上传 64 MiB、上游生成响应 256 MiB，`multipart` 生成请求 192 MiB。

## 能力协商

`GET /api/health` 返回 `{"ok": true, "capabilities": {...}}`。前端连接旧后端时按这些字段决定放行哪些流程。

| 能力字段 | 含义 |
| --- | --- |
| `proxy_generation_status_only` | 支持 `status_only=true` 轻量轮询 |
| `image_generation_options_v2` | 支持自动尺寸等新参数 |
| `image_editing_v1` | 支持遮罩编辑 |
| `image_conversation_v1` | 支持连续会话修改 |
| `provider_model_lists_v1` | 支持单条配置保存多个模型 |
| `managed_provider_accounts_v1` | 支持托管账号 |
| `admin_console_v1` | 支持独立管理后台接口 |
| `managed_provider_templates_v1` | 支持托管服务商模板 |
| `gallery_import_conflict` | 支持模板包导入冲突策略 |

涉及图像编辑或多模型配置的同步会先检查对应能力：缺 `image_editing_v1` 时前端拒绝同步并提示同时升级前后端，缺 `provider_model_lists_v1` 时同样拒绝，以免模型列表在写入过程中被丢弃。这类流程会明确报错，不会静默忽略字段。

## 健康检查

| 方法 | 路径 | 作用 | 权限 |
| --- | --- | --- | --- |
| `GET` | `/api/health` | 存活检查与能力声明 | 公开 |

## 认证

| 方法 | 路径 | 作用 | 权限 |
| --- | --- | --- | --- |
| `POST` | `/api/auth/register` | 注册账号；首个管理员建立前注册可直接使用，之后进入待审批 | 公开 |
| `GET` | `/api/auth/check-username` | 用户名可用性 | 公开 |
| `GET` | `/api/auth/setup-status` | 是否仍可用初始化口令创建首个管理员 | 公开 |
| `POST` | `/api/auth/bootstrap-admin` | 用 `MEW_ADMIN_TOKEN` 把当前登录账号升级为管理员 | 已登录 |
| `POST` | `/api/auth/login` | 登录并轮换 Session ID | 公开 |
| `POST` | `/api/auth/logout` | 删除当前会话 | 公开（无会话时为无害成功） |
| `GET` | `/api/auth/me` | 当前账号信息，未登录时 `user` 为 `null` | 公开 |
| `POST` | `/api/auth/change-password` | 修改密码并让旧会话失效 | 已登录 |

## 管理员

以下接口全部要求已审批的管理员身份。

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| `GET` | `/api/admin/users` | 服务端搜索、状态/角色筛选、排序与分页 |
| `POST` | `/api/admin/users/batch` | 当前页批量批准、禁用、恢复、删除，单次最多 100 个 ID |
| `POST` | `/api/admin/users/export` | 安全 CSV 导出 |
| `POST` | `/api/admin/users/reset-password` | 重置临时密码、清除锁定并递增会话版本 |
| `POST` | `/api/admin/users/approve`、`/disable`、`/restore`、`/delete` | 单账号状态操作 |
| `GET` | `/api/admin/audit` | 审计日志分页查询 |
| `POST` | `/api/admin/managed-users` | 创建托管账号，返回仅显示一次的临时密码 |
| `POST` | `/api/admin/managed-users/{user_id}/reset-password` | 重置托管账号临时密码 |
| `POST` | `/api/admin/managed-users/{user_id}/providers` | 为托管账号新增服务商配置 |
| `GET` | `/api/admin/managed-providers` | 托管配置查询 |
| `POST` | `/api/admin/managed-providers/bulk-credentials` | 同协议与接口模式的原子批量凭据更新 |
| `POST`、`DELETE` | `/api/admin/managed-providers/{config_id}` | 编辑或删除配置 |
| `POST` | `/api/admin/managed-providers/{config_id}/enabled` | 启停配置 |
| `GET`、`POST` | `/api/admin/managed-provider-templates` | 模板列表与创建 |
| `POST`、`DELETE` | `/api/admin/managed-provider-templates/{template_id}` | 编辑或删除模板 |
| `POST` | `/api/admin/managed-provider-templates/{template_id}/enabled` | 启停模板 |
| `POST` | `/api/admin/managed-provider-templates/{template_id}/assign`、`/assign-preview` | 分配模板到账号，以及分配前预览 |
| `POST` | `/api/admin/managed-provider-templates/{template_id}/sync`、`/sync-preview` | 同步模板到关联账号，以及同步前预览 |

## 同步与数据管理

以下接口要求已审批登录用户。

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| `POST` | `/api/sync/push` | 推送本地快照，服务器合并后返回结果 |
| `GET` | `/api/sync/pull` | 拉取服务器快照与 checkpoint |
| `POST` | `/api/sync/merge-preview` | 只计算合并结果与各类数量，不写库 |
| `GET` | `/api/data/stats` | 云端图片数量、占用空间、模板和快照状态 |
| `POST` | `/api/data/clear` | 按 scope 清除同步数据、服务商模板或全部云端数据 |

## 服务商与生成

| 方法 | 路径 | 作用 | 权限 |
| --- | --- | --- | --- |
| `GET` | `/api/providers/templates` | 内置模板加当前用户模板 | 公开，未登录只返回内置模板 |
| `POST` | `/api/providers/templates` | 导入用户服务商模板 | 已审批 |
| `POST` | `/api/providers/generate` | 提交代理生成任务，返回任务 ID | 游客（需开启 `MEW_GUEST_PROXY`）或已审批 |
| `POST` | `/api/managed/providers/generate` | 用服务端托管配置提交强制代理任务 | 托管账号 |
| `GET` | `/api/managed/providers` | 托管账号的非敏感配置摘要 | 托管账号 |
| `POST` | `/api/managed/providers/{config_id}/model` | 切换管理员允许的当前模型 | 托管账号 |
| `GET` | `/api/providers/generate/{job_id}` | 查询任务状态或读取完整结果 | 以随机任务 ID 为句柄 |
| `DELETE` | `/api/providers/generate/{job_id}` | 停止等待并尽力取消后台任务 | 以随机任务 ID 为句柄 |
| `POST` | `/api/images/fetch` | 上游图片 URL 回填，走安全下载 | 游客（需开启 `MEW_GUEST_PROXY`）或已审批 |

## 图片资源

| 方法 | 路径 | 作用 | 权限 |
| --- | --- | --- | --- |
| `POST` | `/api/assets/upload-init` | 申请上传 token 与对象键 | 已审批 |
| `PUT` | `/api/assets/upload/{token}` | 上传原始字节到 staging | 已审批 |
| `POST` | `/api/assets/complete` | 校验并登记正式资源 | 已审批 |
| `POST` | `/api/assets/presence` | 批量检查资源是否仍在云端 | 已审批 |
| `GET` | `/api/assets/{asset_id}` | 下载自己的图片资源 | 已审批 |

## 模板广场

| 方法 | 路径 | 作用 | 权限 |
| --- | --- | --- | --- |
| `GET` | `/api/gallery/templates` | 模板列表，支持搜索、多标签 AND、排序与分页 | 公开 |
| `GET` | `/api/gallery/templates/{template_id}` | 模板详情 | 公开 |
| `POST`、`DELETE` | `/api/gallery/templates/{template_id}/like` | 点赞与取消点赞 | 公开，按账号或访客 Cookie 计数 |
| `GET` | `/api/gallery/tags` | 标签列表 | 公开 |
| `GET` | `/api/gallery/assets/{asset_id}` | 广场图片资源 | 公开 |
| `GET` | `/api/gallery/assets/{asset_id}/thumbnail` | 广场缩略图 | 公开 |
| `GET` | `/api/admin/gallery/tags` | 管理员标签视图 | 管理员 |
| `POST` | `/api/admin/gallery/templates` | 创建模板 | 管理员 |
| `PUT`、`DELETE` | `/api/admin/gallery/templates/{template_id}` | 编辑或删除模板 | 管理员 |
| `POST` | `/api/admin/gallery/assets` | 上传广场资源 | 管理员 |
| `DELETE` | `/api/admin/gallery/assets/{asset_id}` | 删除广场资源 | 管理员 |
| `GET`、`POST` | `/api/admin/gallery/export` | 完整备份，或按分类与标签筛选导出模板包 | 管理员 |
| `POST` | `/api/admin/gallery/export-preview` | 预览匹配的模板数、去重原图数与大小 | 管理员 |
| `POST` | `/api/admin/gallery/import` | 合并导入模板包 | 管理员 |

## 同步接口语义

同步 schema 版本当前是 `3`，记录按稳定 ID 与 `updated_at` 合并，一般冲突取较新的时间；图片记录时间相同时优先保留带远程对象键的数据。删除操作会生成 `Config`、`Task`、`Thread` 或 `Asset` 墓碑，墓碑与实体按时间竞争，时间相同时删除优先，并且长期保留，以免长期离线的设备把已删除记录重新上传。收藏文件夹按稳定 ID 和自身更新时间合并，删除文件夹用独立墓碑。

图片原文件逐张上传，不放进最终同步 JSON；每成功上传一张就立即保存远程对象键，中途失败后下次同步只处理剩余图片。当前设备缺少某张原图时同步不会整体中止，而是先按资产 ID、`SHA-256` 和云端索引恢复，只有两边都缺失才提示该资产无法恢复。

## 生成代理的任务化

`POST /api/providers/generate` 提交后立即返回任务 ID，前端不再靠单个长连接等待上游。轮询语义如下：

| 请求 | 返回 |
| --- | --- |
| 任务排队或运行中 | `202 Accepted`，`status` 为 `queued` 或 `running` |
| 任务成功 | `200`，带完整结果；`status_only=true` 时只返回状态与 `result_byte_len`，不含图片正文 |
| 任务失败 | `200`，`status` 为 `failed` 并在 `error` 中给出原因 |
| 任务不存在或结果过期 | `404` |

结果 TTL 为 10 分钟，`DELETE` 会移除记录并尽力中止后台任务。服务端最多保留 20 个活动任务，实际执行并发由自适应字节预算决定，而不是固定并发数。前端取得处理预算后先用 `status_only=true` 轻量轮询，再领取完整结果。

> ℹ️ **任务 ID 就是访问句柄**
>
> 查询与取消接口不做会话校验，任务 ID 由两个随机 UUID 拼接而成，这个 ID 本身就是访问凭据；不要把它写进公开日志或分享出去。

## 错误与限流

| 状态码 | 典型触发条件 |
| --- | --- |
| `400` | 参数或资源校验失败、云端配额已满、上传大小/哈希/图片类型不符 |
| `401` | 未登录、账号待审批、首次登录未改密、需要管理员权限、托管账号调用普通生成接口、部署已关闭游客代理 |
| `403` | 上游目标被安全策略拒绝，`code` 为 `provider_target_blocked` |
| `404` | 生成任务不存在或结果已过期、资源不存在 |
| `429` | 游客并发或频率超限、注册/登录限流、活动生成任务已满 |
| `502` | 上游请求失败 |

可确证的 `code` 取值包括 `provider_target_blocked`、`guest_proxy_concurrency_limit`、`guest_proxy_rate_limit`、`auth_rate_limited`、`device_registration_limit` 和 `proxy_generation_queue_full`。限流响应带 `retry_after_seconds` 与 `Retry-After` 头。

## 相关页面

- [架构与设计原则](architecture.md)：`shared` 契约与数据分层
- [安全边界](security.md)：SSRF 防护、上传校验与托管账号隔离
- [手动云同步](sync.md)：从使用角度理解合并与墓碑
---

[← 文档目录](README.md) · [上一页：安全边界](security.md) · [下一页：已知边界与路线图](roadmap.md)
