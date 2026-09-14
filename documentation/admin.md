# 管理后台与审计日志

> 管理员专属的全屏后台，用户、托管账号、模板与审计日志都从这里操作。

管理相关操作集中在一个管理员专属的全屏后台里，个人设置页不再承载用户管理入口。界面只是便利层，所有接口仍由后端校验「已审批的管理员」身份。

## 入口与可见性

- 后台地址是 Hash 深链形式：`#/admin`，侧栏页面为 `#/admin/users`、`#/admin/managed`、`#/admin/providers`、`#/admin/audit`。
- 深链可以直接打开和分享给其他管理员，也支持浏览器后退。
- 普通用户和游客的界面不会渲染入口；即使手动改 Hash 或直接请求接口，后端也会拒绝。
- 后台支持刷新，刷新后停留在当前侧栏页面。

## 后台结构

| 侧栏 | 用途 |
| --- | --- |
| 用户 | 列表、筛选、排序、批量操作、删除、CSV 导出 |
| 托管账号 | 创建托管账号、重置临时密码、维护其服务商配置 |
| 服务商模板 | 建立模板、分配、预览并同步到关联账号 |
| 审计日志 | 查询已执行的管理操作 |

## 用户列表

列表由服务端完成搜索、筛选和排序，不是前端过滤当前页：

| 能力 | 说明 |
| --- | --- |
| 搜索 | 按关键词在服务端查询 |
| 筛选 | 按状态与角色筛选，也可按账号类型区分普通与托管 |
| 排序 | 选择排序字段与升降序 |
| 分页 | 每页 `20` / `50` / `100` 三档，表头固定不随滚动消失 |
| 聚合列 | 一次查询同时返回每人的图片数与托管配置数 |

列表里管理员行不提供禁用与删除按钮：管理员不能禁用或删除当前登录的自己，批量操作也不能包含管理员账号。

## 批量操作

批量选择只作用于当前页。支持的操作包括批准、禁用、恢复和删除，以及安全 CSV 导出。导出文件带 UTF-8 BOM，单元格会阻止公式注入，避免用表格软件打开时被当作公式执行。

> ⚠️ **删除要输入确认文字**
>
> 删除单个用户时要输入该用户名；批量删除时要输入「删除 N 个用户」，其中 N 与选中数量一致。批量请求最多接受 100 个用户 ID，且 ID 不能重复。确认文字不匹配时后端直接拒绝。

删除用户会同时清理该用户的同步快照、服务商模板、图片索引、上传凭证与 Local/S3 对象命名空间。

## 审计日志

在后台内成功完成的用户、托管配置和服务商模板操作都会写入永久审计日志，日志保留操作者、操作目标以及批次快照。日志用于事后追溯，因此不记录 API Key、完整地址、提示词、图片、Base64 或密文。日志页面支持按关键词和操作类型查询。

## 状态一致性

列表项的状态键包含状态、更新时间或修订号。批准、禁用、恢复、启停与密码重置完成后会立即刷新对应数据，不会因为复用了旧的 DOM 行而显示过期状态。

## 相关管理接口

需要写脚本或做集成时，可以直接对照后端路由。以下是主要路径：

| 方法 | 路径 |
| --- | --- |
| `GET` | `/api/admin/users?page=&limit=&q=&status=&role=&account_kind=&sort=&order=` |
| `POST` | `/api/admin/users/batch`、`/api/admin/users/export`、`/api/admin/users/reset-password` |
| `POST` | `/api/admin/users/approve`、`/api/admin/users/disable`、`/api/admin/users/restore`、`/api/admin/users/delete` |
| `GET` | `/api/admin/audit` |
| `POST` | `/api/admin/managed-users`、`/api/admin/managed-users/{user_id}/reset-password`、`/api/admin/managed-users/{user_id}/providers` |
| `GET` | `/api/admin/managed-providers` |
| `POST` `DELETE` | `/api/admin/managed-providers/{config_id}` |
| `POST` | `/api/admin/managed-providers/bulk-credentials`、`/api/admin/managed-providers/{config_id}/enabled` |
| `GET` `POST` | `/api/admin/managed-provider-templates`、`/api/admin/managed-provider-templates/{template_id}` |
| `POST` | `/api/admin/managed-provider-templates/{template_id}/enabled`、`/assign`、`/assign-preview`、`/sync`、`/sync-preview` |

> 💡 **以运行中的版本为准**
>
> 路由会随版本增补。升级镜像后，建议对照仓库的 `backend/src/main.rs`、`admin_console.rs` 与 `managed_providers.rs` 里的 `route(` 调用确认一遍。

## 相关页面

- 审批状态与登录规则：[账号、审批与登录防护](accounts.md)
- 托管账号与模板的完整语义：[托管账号与服务商模板](managed.md)
- 认证相关接口：[后端 API](api.md)
---

[← 文档目录](README.md) · [上一页：托管账号与服务商模板](managed.md) · [下一页：后端资源存储](storage.md)
