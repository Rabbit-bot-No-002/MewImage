# 排障

> 部署与使用中最常见的报错：现象、原因和对应的处理动作。

下面按「现象 → 原因 → 处理」列出常见问题，报错细节多在 `docker compose logs -f app` 里。

## 容器反复重启，日志出现 `Permission denied (os error 13)`

**现象**：容器不断重启，日志报 `Permission denied (os error 13)`，前面的 OpenResty/Nginx 报 `502 Bad Gateway`。

**原因**：容器固定以 `10001:10001` 运行，挂载到 `/data` 的宿主机目录却属于 `root`，后端无法创建临时目录、SQLite WAL 或图片资源。`502` 是后端没起来的连带结果。

**处理**：停止容器，修正目录归属后重启：

```bash
docker compose down
sudo mkdir -p ./data/assets ./data/.tmp
sudo chown -R 10001:10001 ./data
sudo find ./data -type d -exec chmod 750 {} \;
sudo find ./data -type f -exec chmod 600 {} \;
docker compose up -d
```

> 🛑 **不要把应用改回 root**
>
> 这个报错的处理方式是修正目录归属。SELinux 主机还要加私有标签 `./data:/data:rw,Z`；CIFS、NFS 或 NAS 共享要在共享 ACL 或挂载参数里授予 UID/GID `10001` 写权限。

## 反向代理返回 504 或超时

**现象**：NPM、Nginx 或 OpenResty 在生成过程中返回 `504 Gateway Time-out`。

**原因**：旧版本用单个长连接等待上游，耗时数分钟的任务会撞上反代的读取超时；新版本已改为后台任务加短轮询。

**处理**：通常不需要为反代放大读取超时。先确认镜像已更新到 `v1.1.1`，再检查反代到后端的连通性与后端日志；仍报 504 时优先排查上游响应速度。

## 直接通过服务器 IP 打不开

**现象**：访问 `http://服务器IP:3188` 打不开，或页面能开而请求全部失败。

**原因**：端口绑定范围、CORS 来源、Cookie 安全标记要同时成立。

**处理**：按直连方式成组修改 `.env` 后重建容器：

```dotenv
MEW_HOST_BIND=0.0.0.0
MEW_ALLOWED_ORIGINS=http://你的服务器IP:3188
MEW_PUBLIC_BASE_URL=http://你的服务器IP:3188
MEW_SESSION_SECURE=false
MEW_TRUST_PROXY_HEADERS=false
```

`MEW_HOST_BIND` 保持 `127.0.0.1` 时只有宿主机本机能连上；`MEW_ALLOWED_ORIGINS` 缺端口会被判为来源不匹配；HTTP 直连却设 `MEW_SESSION_SECURE=true` 会让 Cookie 被丢弃。

## 登录后 Cookie 不生效，一直跳回登录

**现象**：能提交登录表单，但状态没有变化，刷新后又回到未登录。

**原因**：Cookie 的 Secure 标记与站点协议不匹配，或后端没识别到代理转发的真实来源。

**处理**：HTTPS 站点设 `MEW_SESSION_SECURE=true`，HTTP 直连设 `false`。经过反代时确认 `MEW_TRUST_PROXY_HEADERS=true`，且 `MEW_TRUSTED_PROXY_CIDRS` 填的是实际反代 IP/CIDR，不匹配的连接仍按直连处理。反向代理要主动覆盖 `X-Real-IP` 和 `X-Forwarded-For`。

## 浏览器控制台报 CORS 错误

**现象**：请求被浏览器拦截，控制台提示来源不在允许列表。

**原因**：`MEW_ALLOWED_ORIGINS` 与浏览器实际 Origin 不完全一致。

**处理**：写法必须是 `协议://主机:端口`，逐项核对：

| 常见错法 | 正确写法 |
| --- | --- |
| `https://img.example.com/` 末尾多一个斜杠 | `https://img.example.com` |
| `http://服务器IP` 漏掉端口 | `http://服务器IP:3188` |
| 只填域名没写协议 | `https://img.example.com` |

正式上线只保留真实站点地址，多个地址用英文逗号分隔。

## 上游被安全策略拦截

**现象**：生成时报错说明目标主机被拒绝，日志里能看到该主机的解析地址。

**原因**：后端默认拒绝本机、私网、链路本地地址、非 HTTP/HTTPS scheme 和解析后指向危险地址的目标；图片回填还会逐跳检查重定向。

**处理**：公网第三方中转站不需要白名单，但必须解析到安全公网地址且默认使用 HTTPS。开关边界如下：

| 变量 | 默认 | 边界 |
| --- | --- | --- |
| `MEW_ENFORCE_HOST_WHITELIST` | `false` | 设为 `true` 后只允许 `MEW_TRUSTED_HOSTS` 里列出的上游 |
| `MEW_TRUSTED_HOSTS` | 空 | 严格白名单模式下的固定上游域名 |
| `MEW_ALLOW_HTTP_UPSTREAM` | `false` | 设为 `true` 才允许明文 `http://` 上游 |

无论白名单是否开启，`localhost`、回环地址、私网 IP 和链路本地地址都默认被拒绝；API 是公网域名但图片解析到私网或 Fake-IP 时，图片下载同样失败。

## 本机开发想访问本机或局域网上游

**现象**：开发机上指向 `127.0.0.1`、局域网地址或 Fake-IP 的上游被拒。

**原因**：默认策略刻意拒绝这些地址。

**处理**：两个开发开关都必须先让后端监听环回地址：

| 变量 | 生效条件与范围 |
| --- | --- |
| `MEW_ALLOW_LOOPBACK_UPSTREAM` | 只允许显式的 `127.0.0.1`、`::1`、`localhost`、`localhost.localdomain`；只允许后端自身监听环回地址时生效；不会放开 `10.x`、`172.16–31.x`、`192.168.x`；明文环回上游还要同时开 `MEW_ALLOW_HTTP_UPSTREAM` |
| `MEW_DEV_BYPASS_UPSTREAM_SSRF` | 跳过生成、图片下载（含重定向）的主机与解析 IP 检查，可用于局域网上游或 Fake-IP DNS；同样只允许后端监听环回地址时生效；HTTP 仍需 `MEW_ALLOW_HTTP_UPSTREAM=true` |

后端绑定 `0.0.0.0`、`[::]` 或局域网地址时会忽略 `MEW_DEV_BYPASS_UPSTREAM_SSRF` 并记录警告。Docker 默认监听 `0.0.0.0:3000`，它们不会放宽容器部署；生产建议保持 `false`。

## 第 21 个任务提示容量已满

**现象**：再提交任务时提示已有 20 个活动任务。

**原因**：前后端都最多保留 20 个活动任务卡，第 21 个会被拒绝；实际并发还受内存预算限制。

**处理**：等待部分任务完成，或在结果画廊停止不再需要的任务后再提交。

## 上传图片被拒

**现象**：上传参考图或云端图片时报格式、尺寸或大小不合法。

**原因**：上传路径有多层硬限制。

| 限制 | 数值 |
| --- | --- |
| 云端资源格式 | 仅 `image/png`、`image/jpeg`、`image/webp`，且魔数要与声明类型一致 |
| 单个云端图片 | `MEW_MAX_ASSET_MIB`，默认 64 MiB |
| 单张参考图或遮罩 | 32 MiB |
| 参考图与遮罩合计 | 160 MiB |
| 单次生成参考图数量 | 10 张 |
| 编辑遮罩格式 | 必须为 PNG |
| 模板广场图片最长边 | 4096px |

**处理**：确认格式在支持范围内、单文件与合计大小未超限；空文件会被直接拒绝。云端配额用满时同样拒绝新增，但已有资源仍可删。

## 同步被阻止：旧后端缺少能力字段

**现象**：点击同步时报错，提示当前后端不支持图像编辑数据同步或多模型服务商配置同步。

**原因**：前端读取 `/api/health` 的能力字段做协商。当前端存在编辑任务或配置了多个模型、而后端未声明 `image_editing_v1` 或 `provider_model_lists_v1` 时，同步会被明确阻止，避免静默丢字段。

**处理**：把前后端升级到同一版本再同步，升级后端后不需要额外交互。

## 相关页面

[部署](deploy.md) · [环境变量参考](config.md) · [后端资源存储](storage.md) · [快速开始](quickstart.md)
---

[← 文档目录](README.md) · [上一页：后端资源存储](storage.md) · [下一页：本地开发与联调](local-dev.md)
