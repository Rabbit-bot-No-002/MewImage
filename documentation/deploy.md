# 部署

> 用 Docker Compose 跑起喵图：目录权限、端口暴露、反向代理与升级。

服务器上只需要 Docker 与 Docker Compose。镜像内已包含前端、后端和 SQLite，不用额外准备数据库，也不用单独部署静态站点。

## 运行前提

| 项目 | 要求 |
| --- | --- |
| 宿主机 | 已安装 Docker 与 Docker Compose |
| 架构 | 当前镜像只提供 `linux/amd64`；ARM64 服务器或 NAS 需要等后续多架构镜像 |
| 端口 | 容器内固定监听 `3000`，宿主机默认映射到 `3188` |
| 数据目录 | 宿主机 `./data`，同时存放 SQLite 与 Local 模式图片 |

## Compose 文件做了什么

`docker-compose.yml` 只定义一个服务 `app`：

| 配置项 | 值 | 说明 |
| --- | --- | --- |
| `image` | `${MEW_DOCKER_IMAGE}` | 默认 `mewlab/mewimage:latest`，固定版本时改成 `mewlab/mewimage:1.1.1` |
| `ports` | `${MEW_HOST_BIND:-127.0.0.1}:${MEW_HOST_PORT}:3000` | 容器内固定 `3000`，`MEW_HOST_PORT` 只改宿主机一侧 |
| `env_file` | `.env` | 应用配置从同目录 `.env` 注入，Compose 文件不用改 |
| `volumes` | `./data:/data:rw` | SQLite、Local 图片与临时文件 |
| `environment` | 固定四项 | `MEW_LISTEN=0.0.0.0:3000`、`MEW_DATABASE_URL=sqlite:///data/mew-image.db`、`MEW_FRONTEND_DIST=/app/frontend/dist-app`、`MEW_LOCAL_ASSET_DIR=/data/assets` |

容器还启用了只读根文件系统、`cap_drop: ALL`、`no-new-privileges`、`/tmp` tmpfs、`pids_limit: 256` 和 `${MEW_MEMORY_LIMIT}` 内存上限，日志使用 json-file 驱动并轮转。

## 数据目录要自己建

容器以固定的非 root 用户 `10001:10001` 运行。Docker 自动创建 `./data` 时目录通常归 `root`，容器随即退出并报 `Permission denied (os error 13)`。首次启动前先建目录并改归属：

```bash
chmod 600 ./.env
sudo mkdir -p ./data/assets ./data/.tmp
sudo chown -R 10001:10001 ./data
sudo find ./data -type d -exec chmod 750 {} \;
sudo find ./data -type f -exec chmod 600 {} \;

docker compose up -d
docker compose ps
docker compose logs -f app
```

> ⚠️ **不要把应用改回 root**
>
> `Permission denied` 的处理方式是修正宿主机目录归属，而不是去掉 `USER 10001:10001`。Windows 与 macOS 的 Docker Desktop bind mount 通常不需要手工改 UID/GID。

SELinux 主机需要私有标签 `./data:/data:rw,Z`；CIFS、NFS 或 NAS 共享要在共享 ACL 或挂载参数里授予 UID/GID `10001` 写权限。

## 暴露范围与反向代理

Compose 默认只绑定宿主机 `127.0.0.1:3188`，适合宿主机上直接运行的 Nginx 或 Caddy。

- 宿主机上的 Nginx/Caddy：反向代理到 `127.0.0.1:3188` 即可。
- 容器化的 Nginx Proxy Manager：无法直接访问宿主机回环地址，应通过 Compose override 加入共享 Docker 网络并直接代理 `app:3000`，或只向受防火墙保护的地址发布端口。
- 直接通过服务器 IP 访问：把 `MEW_HOST_BIND` 显式改成 `0.0.0.0`，并自行用防火墙限制来源。

`MEW_HOST_BIND` 决定谁能连到这个端口。改成 `0.0.0.0` 之前先想清楚：同网段的任何机器都能直达后端，账号限流按直连 IP 计算，绕过后端前面的反代也就绕过了那层访问控制。

## HTTPS 与代理头

走 HTTPS 时这几项要一起调：

```dotenv
MEW_SESSION_SECURE=true
MEW_TRUST_PROXY_HEADERS=true
MEW_TRUSTED_PROXY_CIDRS=反向代理的精确IP或CIDR
```

`MEW_SESSION_SECURE=true` 让 Session Cookie 带上 Secure 标记，HTTPS 站点不设会导致登录后 Cookie 不生效。`MEW_TRUST_PROXY_HEADERS` 只有在后端端口无法绕过反向代理直接从公网访问时才能开启，否则客户端可以自己伪造转发头；开启后必须用 `MEW_TRUSTED_PROXY_CIDRS` 限定真实反向代理，不匹配的连接仍按直连 IP 处理。反向代理还要主动覆盖 `X-Real-IP` 和 `X-Forwarded-For`，不要原样透传客户端提交的同名请求头。

`MEW_PUBLIC_BASE_URL` 是对外分享用的公开站点来源，用于生成绝对 Open Graph 图片地址和 canonical URL。只写协议、域名和可选端口，例如 `https://img.example.com`；带路径、账号、查询参数、片段或末尾斜杠都会被拒绝。留空时页面保留相对图片地址，浏览器访问不受影响，但部分社交平台可能抓不到预览图。

## 升级与固定版本

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs -f app
```

改过 `.env` 后要重建容器才生效：

```bash
docker compose up -d --force-recreate
```

> 💡 **为什么 restart 不管用**
>
> 环境变量在容器创建时就固定了。`docker compose restart` 只是原地重启旧容器，不会重新读取 `.env`。

固定版本时把 `MEW_DOCKER_IMAGE` 改成 `mewlab/mewimage:1.1.1`，避免 `latest` 在不知不觉中升级。从旧版 root 容器升级时，先 `docker compose down`，再按上面的命令递归迁移 `./data` 权限。

## 相关页面

[快速开始](quickstart.md) · [环境变量参考](config.md) · [后端资源存储](storage.md) · [排障](troubleshooting.md)
---

[← 文档目录](README.md) · [上一页：快速开始](quickstart.md) · [下一页：环境变量参考](config.md)
