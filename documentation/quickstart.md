# 快速开始

> 一台装了 Docker 的机器，四条命令，五分钟内打开工作台。

前置条件只有一条：服务器上装好 Docker 与 Docker Compose。镜像内已经包含前端、后端和 SQLite，不需要额外数据库。

## 1. 取部署文件

```bash
mkdir -p mewimage && cd mewimage

curl -LO https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/docker-compose.yml
curl -o .env https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/.env.example
```

国内网络可直接换成 Gitee 镜像：

```bash
curl -fL -O https://gitee.com/ln-q/MewImage/raw/main/docker-compose.yml
curl -fL -o .env https://gitee.com/ln-q/MewImage/raw/main/.env.example
```

## 2. 生成两个密钥

```bash
openssl rand -hex 32      # 填给 MEW_AUTH_SECRET
openssl rand -base64 32   # 填给 MEW_ADMIN_TOKEN
```

编辑 `.env`，至少改三项：

```dotenv
MEW_AUTH_SECRET=第一条命令的输出
MEW_ADMIN_TOKEN=第二条命令的输出
MEW_ALLOWED_ORIGINS=https://你的正式域名
```

> ⚠️ **MEW_AUTH_SECRET 一旦上线就不要更换**
>
> 它参与设备与 IP 摘要和认证保护。中途更换会让已签发的会话与设备摘要失效。

上了 HTTPS 之后再补这几项：`MEW_SESSION_SECURE=true`、`MEW_TRUST_PROXY_HEADERS=true`（配合 `MEW_TRUSTED_PROXY_CIDRS`），以及 `MEW_PUBLIC_BASE_URL=https://你的正式域名`——最后这个用来生成分享预览图的绝对地址，留空时页面照常访问，但社交平台可能抓不到预览图。

## 3. 建数据目录并启动

容器以固定的非 root 用户 `10001:10001` 运行。**先自己创建 `./data`**，否则 Docker 自动创建的目录会属于 `root`，容器启动后日志会报 `Permission denied (os error 13)`。

```bash
chmod 600 ./.env
sudo mkdir -p ./data/assets ./data/.tmp
sudo chown -R 10001:10001 ./data

docker compose up -d
docker compose ps
docker compose logs -f app
```

`docker compose up -d` 之后如果又改了 `.env`，必须重建容器才生效：

```bash
docker compose up -d            # 检测到配置变化会自动 Recreate
docker compose pull && docker compose up -d   # 镜像也更新过时
```

> 💡 **为什么 restart 不管用**
>
> 环境变量在容器创建时就固定了。`docker compose restart` 只是把旧容器原地重启，不会重读 `.env`。

## 4. 创建第一个管理员

打开站点，走 **设置 → 账号与同步 → 注册 → 使用管理员初始化口令**，填入 `.env` 里的 `MEW_ADMIN_TOKEN`。密码至少 10 位，包含大小写字母、数字和符号。这个账号直接是已审批的管理员；之后普通用户注册会进入待审批状态，需要管理员在管理后台批准。

如果已经注册过普通账号、但库里还没有管理员，可以先登录该账号，再点「将当前账号初始化为管理员」。想确认当前状态：

```bash
curl http://127.0.0.1:3188/api/auth/setup-status
```

`admin_exists` 为 `true` 表示库里已经有管理员，这条初始化路径就不再允许创建第二个。

## 默认端口与暴露范围

- Compose 默认只绑定宿主机 `127.0.0.1:3188`，适合宿主机上直接跑的 Nginx 或 Caddy。
- 容器内固定监听 `3000`，`MEW_HOST_PORT` 只改宿主机一侧。
- 想让宿主机以外的机器直接访问，把 `MEW_HOST_BIND` 显式改成 `0.0.0.0`，并自行用防火墙限制来源。

## 目录结构

```text
mewimage/
├── docker-compose.yml
├── .env
└── data/
    ├── mew-image.db      # SQLite
    └── assets/           # 登录用户的云端图片
```

## 接下来

- 配 HTTPS 反代、调 `MEW_SESSION_SECURE`：[部署](deploy.md)
- 逐项看懂每个环境变量：[环境变量参考](config.md)
- 启动失败、502、权限报错：[排障](troubleshooting.md)
---

[← 文档目录](README.md) · [上一页：喵图文档](README.md) · [下一页：部署](deploy.md)
