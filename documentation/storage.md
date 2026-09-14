# 后端资源存储

> 云端图片存到哪里、配额怎么算、从 Local 换到 S3 怎么迁。

只有已审批登录用户的云端图片走服务器存储。游客使用代理时只做瞬时转发，不在服务器上留下工作区或图片。存到哪里由 `MEW_ASSET_STORE` 决定。

## 三种模式

| 模式 | 存到哪 | 适合谁 |
| --- | --- | --- |
| `local` | `MEW_LOCAL_ASSET_DIR`，Docker 默认 `/data/assets`，对应宿主机 `./data/assets` | 个人 Docker 部署（默认） |
| `s3` | S3 兼容对象存储，自定义 endpoint、region、bucket 和访问密钥 | 已有对象存储，或不想把图片放在本机 |
| `disabled` | 禁用服务器远程资源存储 | 只用浏览器本地的部署 |

`disabled` 下游客本地生成仍可用，但登录同步图片、远程资源上传和跨设备图片恢复都不可用，上传初始化会直接返回明确错误。

`MEW_S3_BUCKET`、`MEW_S3_REGION`、`MEW_S3_ENDPOINT`、`MEW_S3_ACCESS_KEY`、`MEW_S3_SECRET_KEY` 只在 `MEW_ASSET_STORE=s3` 时需要填写；`local` 和 `disabled` 下留空即可。

## 目录里有什么

```text
data/
├── mew-image.db        # SQLite 索引与会话
├── assets/             # Local 模式的云端图片，即对象键 users/{user_id}/assets/...
├── .staging/           # 上传完成前的暂存文件
└── .tmp/               # TMPDIR，等待任务的大型参考图临时文件
```

## 配额

| 变量 | 默认值 | 作用 |
| --- | --- | --- |
| `MEW_MAX_ASSET_MIB` | `64` | 单个云端图片上限 |
| `MEW_USER_ASSET_QUOTA_MIB` | `5120` | 每个已审批用户的总容量上限，即 5 GiB |
| `MEW_GALLERY_ASSET_QUOTA_MIB` | `5120` | 模板广场公共资源总容量 |
| `MEW_GALLERY_ASSET_QUOTA_COUNT` | `20000` | 模板广场公共资源数量 |

用户达到配额后不能再新增，但**已有资源仍可读取、下载和删除**，报错信息也会说明这一点。模板广场使用独立的公共命名空间，配额把派生缩略图一起计入：一张模板图片按原图加缩略图各计一次数量和字节。达到上限后管理员仍可删除旧模板释放空间。

## 对象键规则

Local 与 S3 使用同一套对象键：

```text
users/{user_id}/assets/...
```

- 不同用户使用独立命名空间；后端会校验同步快照里的对象键属于当前用户。
- 新上传以 SHA-256 为主去重依据，Docker 下路径形如 `users/{user_id}/assets/{sha256}.bin`。
- 删除对象前先检查是否还有其他资产记录引用同一对象键。
- Local 路径拼接拒绝 `..`、绝对路径等目录穿越写法。

## 上传凭证与校验

上传分初始化、字节上传、完成确认三步，三步都要求已审批登录用户。

| 环节 | 校验内容 |
| --- | --- |
| 初始化 | 非空、不超过单文件上限、SHA-256 为 64 位十六进制、MIME 为 `image/png`、`image/jpeg` 或 `image/webp` |
| 字节上传 | `Content-Length` 与实际正文一致、SHA-256 匹配、文件魔数与声明类型一致 |
| 完成确认 | 实际大小、SHA-256、魔数/MIME 再次核对后才登记正式资源 |

上传 token 默认 15 分钟过期，完成后立即删除，不能重复消费；过期 token 和孤儿对象由后台清理。上传先写 staging，不存在「SQLite 有记录但对象不存在」的伪成功状态。

模板包与项目包导入另外执行危险路径、重复条目、异常压缩比、解压后总量、资源大小、格式、尺寸与 SHA-256 校验。模板广场图片最长边不能超过 `4096px`。

## 从 Local 迁移到 S3

切换 `MEW_ASSET_STORE` 不会自动搬迁文件，也没有 Local/S3 双读回退。两边对象键相同，所以不用改 SQLite，也不用通过网页重新导出、导入图片。

1. 停止 MewImage，避免迁移期间继续产生新图片或未完成上传。
2. 备份整个 `./data`，包括 `mew-image.db` 和 `assets/`。
3. 把 `./data/assets` 的内容同步到 S3 Bucket 根目录，保留原始相对路径。
4. 确认 Bucket 里的路径直接以 `users/` 开头，而不是 `assets/users/`。
5. 保留原 SQLite 数据库，填好 S3 变量并把 `MEW_ASSET_STORE` 改成 `s3`。
6. 重启后检查历史图片读取、新图上传、删除和跨设备同步。
7. 运行稳定后再清理本地图片，建议至少保留一段时间作为回滚备份。

```bash
aws s3 sync ./data/assets s3://你的Bucket \
  --endpoint-url https://你的S3端点
```

MinIO Client 可以改用 `mc mirror ./data/assets 你的别名/你的Bucket`。迁移失败时，只要本地文件和 SQLite 还在，把 `MEW_ASSET_STORE` 切回 `local` 并重启即可回滚。

## 相关页面

[部署](deploy.md) · [环境变量参考](config.md) · [手动云同步](sync.md) · [排障](troubleshooting.md)
---

[← 文档目录](README.md) · [上一页：管理后台与审计日志](admin.md) · [下一页：排障](troubleshooting.md)
