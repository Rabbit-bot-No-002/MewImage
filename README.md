# MewImage

当前版本：`v1.1.0`

一个可多设备同步的图片生成平台。
AI Coding 的产物，主要是满足个人使用需求，Docker部署。

- 前端：`Leptos CSR`
- 后端：`Axum + SQLite`
- 资源存储：`本地文件存储`或 `S3 兼容对象存储`
- 模式：本地优先，登录后手动跨设备同步

## 视频演示
bilibili：https://www.bilibili.com/video/BV1njKa6VEm8/?share_source=copy_web&vd_source=81866f08e909626b4220c9656edc09ce  

youtube：https://youtu.be/43DXly6Cw5U?si=57ihgJLuHs3uyjgu

## 当前实现

- 历史、会话、收藏、参考图、界面偏好默认保存在浏览器 IndexedDB；v3 起原图以 Blob 保存，旧 Data URL 会按需迁移
- 登录只是同步增强能力，不是使用前提
- 注册用户默认需要管理员审批，审批通过后才能使用云端同步和服务器资源存储
- 注册与登录带设备/IP 限流；账号连续输错 5 次密码后锁定
- 用户界面会显示当前账号保存在服务器的图片数量；管理员列表会显示每个用户的服务器图片数量
- 支持 `OpenAI Image`、`Nano Banana`、`OpenAI 兼容`
- 游客默认可以使用任意通过公网地址校验的 HTTPS 标准图像中转站和结果图代理；`CustomHttp` 仅向已审批登录用户开放
- OpenAI Image 提供“关闭 / API 原生 / 本地去背”三种背景模式；中转站不支持官方透明参数时，可在浏览器中生成真正带 Alpha 通道的 PNG 或 WebP
- 代理生图使用后台任务和轻量状态轮询；上传参考图后会立即释放浏览器准备预算，结果就绪时再按 FIFO 领取处理预算
- 支持本机记忆的生图队列模式：提交后画廊立即显示等待卡，可继续修改提示词、参考图和参数并发提交；队列模式与连续修改模式互斥
- 支持“经典 Mew / 极光星轨 / 液态玻璃”主题，以及日间、夜间、跟随系统和三档页面装饰强度
- “设置 → 外观”可上传全局自定义背景，并调整填充、位置、透明度、模糊度、主题遮罩和背景图层；还可降低页面卡片不透明度，让背景保持可见
- 登录态远程资源上传可使用本地文件存储；需要云对象存储时可切换到 S3 兼容模式
- 数据管理支持完整工作区 ZIP、单会话项目包导出与恢复、合并导入、分类清除，以及登录用户自助查看和清除自己的云端数据
- 新增“模板广场”主视图：公开模板支持搜索、AND 多标签筛选、最新/热门排序、游客点赞、外链分享、收藏快照和一键填入工作台；创建和管理模板仅向管理员开放

单会话项目包会保存该会话的草稿、生成任务、参数快照、结果原图和参考图，不包含服务商配置、API Key 或收藏状态。导入时始终创建独立的新会话副本，可用于归档项目并在以后继续生图。

本地去背采用纯绿/纯洋红色键背景后处理，不是语义分割模型。处理成功时只保存透明结果，不再额外保留纯色原图；处理失败时保留原图并在任务详情记录原因。旧测试版本曾保存的隐藏纯色原图会在启动时自动清理。第三方算法许可见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。

队列模式最多保留 20 个活动任务卡。新后端会把浏览器内存占用拆成“参考图准备、等待上游、结果处理”三个阶段：代理任务等待上游时不占用大图预算，已完成结果按 FIFO 优先于尚未提交的任务逐张处理。预算按 `deviceMemory × 64 MiB` 自动计算并限制在 192–512 MiB；Firefox 等不提供该信息的浏览器使用 256 MiB。单阶段超过软预算时允许独占运行，不会永久卡在“等待浏览器内存预算”。Direct 和旧后端兼容链路仍在请求全程持有完整预算，避免响应到达时并发解码造成内存峰值。

等待卡会显示“服务端排队、等待上游、等待结果处理、本地去背、保存结果”等真实阶段，可单独停止，设置栏也可停止全部；确认取消后卡片会直接移除，但已经到达上游的请求仍可能产生费用。生成结果会先逐张暂存到 IndexedDB，全部完成后再一次性更新画廊；浏览器异常重启时可恢复完整结果或已落盘的部分结果。第 21 个活动任务会被明确拒绝，不会继续占用内存。

图片 payload 写入使用可重试的单飞队列，只有 IndexedDB 事务成功后才会从队列移除；浏览器存储配额不足时会保留当前内存结果并显示错误。画廊按需创建 Blob URL，缓存淘汰或页面卸载时释放，不会因为清理内存缓存而删除 IndexedDB 原图。

如果浏览器本地快照暂时读取失败，页面会进入写保护并要求刷新重试，不会把默认空工作区覆盖到原有 IndexedDB 数据；旧版内嵌图片迁移失败时也会保留最后一份可恢复 payload。

自定义背景接受静态 PNG、JPEG 和 WebP，原文件最大 15 MiB、解码后不超过 5000 万像素。浏览器会将最长边限制到 4096px，并统一转换为质量 0.8 的透明 WebP；处理后超过 8 MiB 时不会保存。背景可以放在主题装饰下方，也可以覆盖主题装饰；页面卡片不透明度可在 20%–100% 之间调整。背景原文件独立存入 IndexedDB，不会进入偏好 JSON；登录用户点击“立即同步”时才会上传。完整工作区 ZIP 包含主题背景，单会话项目包不包含全局外观设置。

“极光星轨”采用独立的深空星轨仪表风格：日间为冰蓝星图纸，夜间为深靛天文台，主操作使用青蓝至紫罗兰渐变，粉色只作为品牌和星体点缀。面板使用高 Alpha、约 18px 的磨砂效果，与液态玻璃的高透明观感明显区分；装饰档位可切换关闭、柔和或标准，动画仅使用低开销的位移和透明度变化，并遵循系统的减少动态效果设置。

“液态玻璃”使用低模糊、高透明面板和折射光纹，让自定义背景在卡片后方保持可辨认。大型面板只使用约 5px 的轻度模糊，重复画廊卡片不单独模糊；不支持 `backdrop-filter` 的浏览器会自动使用更高不透明度的安全回退样式。

## 模板广场

页面左上角可在“工作台 / 模板广场”间切换，收藏按钮仍保留在原位置，Logo 与标题保持居中。模板广场默认显示最新发布内容，每页 24 项；搜索覆盖标题、提示词和标签，多标签筛选要求模板同时具备全部所选标签。标签筛选采用“分类 → 标签”的两级弹出菜单；管理员编辑模板时可直接选择分类、搜索并多选已有标签，也可分别输入分类和新标签，不需要手写路径。新标签支持用中英文逗号、顿号、分号或换行批量添加，普通空格会保留；历史 `分类/标签` 和无分类标签继续兼容，无分类内容统一显示在“未分类”。分享链接使用 `/?view=templates&template={模板UUID}`，无需额外的前端路由配置。

点击“使用模板”只会下载并校验参考图、填充原始提示词和兼容参数，然后返回工作台等待用户确认，不会自动提交生成或产生费用。当前服务商与推荐服务商不一致时保留当前配置，并显示推荐模型差异。点击“收藏”会在浏览器中建立包含预览、参考图和参数的独立本地快照，因此模板下架后仍可使用，但会额外占用 IndexedDB 空间。

管理员可以新建模板，或从成功结果卡片的发布按钮预填草稿。每个模板最多包含 6 张预览图和 16 张参考图；浏览器统一转换为质量 0.9 的 WebP，预览图最长边 2048px、参考图最长边 4096px。服务端会为模板图片生成最长边 480px 的独立缩略图，列表和紧凑图片条优先加载缩略图，详情主预览及实际复用时再读取原图。草稿和归档内容仅管理员可见，公开用户只能访问已发布模板及其图片。

管理员可单独导出模板广场 ZIP，也可选择合并或全量替换导入。模板包包含模板、标签、非敏感生成参数和经 SHA-256 校验的全部原始图片，不包含账号、API Key、点赞身份或票数；缩略图属于可重建派生数据，导入时由服务端重新生成。全量替换会清空现有点赞并经过两次确认；服务端完整校验 ZIP 路径、大小、数量、格式和哈希后才提交 SQLite 事务，对象存储 staging 记录会在提交时原子转换并由后台回收中断上传。广场资源默认总配额为 5 GiB、20,000 个文件，可通过 `MEW_GALLERY_ASSET_QUOTA_MIB` 和 `MEW_GALLERY_ASSET_QUOTA_COUNT` 调整。

## Docker Compose 一键部署

Docker Hub 镜像：[`mewlab/mewimage`](https://hub.docker.com/r/mewlab/mewimage)

服务器只需要安装 Docker 与 Docker Compose，下面的命令会下载 Compose 和环境变量模板：

```bash
mkdir -p mewimage && cd mewimage

curl -LO https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/docker-compose.yml
curl -o .env https://raw.githubusercontent.com/Rabbit-bot-No-002/MewImage/main/.env.example
```

国内镜像加速
```bash
mkdir -p mewimage && cd mewimage

curl -fL -O https://gitee.com/ln-q/MewImage/raw/main/docker-compose.yml
curl -fL -o .env https://gitee.com/ln-q/MewImage/raw/main/.env.example
```

生成认证密钥和管理员初始化口令：

```bash
openssl rand -hex 32
openssl rand -base64 32
```

编辑 `.env`，至少修改以下三项：

```dotenv
MEW_AUTH_SECRET=第一条命令生成的随机值
MEW_ADMIN_TOKEN=第二条命令生成的随机值
MEW_ALLOWED_ORIGINS=https://你的正式域名
```

如果暂时通过 `http://服务器IP:3188` 直接访问，则把 `MEW_ALLOWED_ORIGINS` 改成该完整来源，并保持：

```dotenv
MEW_HOST_BIND=0.0.0.0
MEW_SESSION_SECURE=false
MEW_TRUST_PROXY_HEADERS=false
```

如果已经通过 Nginx Proxy Manager、Nginx 或 Caddy 配置 HTTPS，则推荐：

```dotenv
MEW_ALLOWED_ORIGINS=https://你的正式域名
MEW_SESSION_SECURE=true
MEW_TRUST_PROXY_HEADERS=true
MEW_TRUSTED_PROXY_CIDRS=反向代理所在的精确IP或CIDR
```

只有当 MewImage 后端端口无法绕过反向代理直接从公网访问时，才能开启 `MEW_TRUST_PROXY_HEADERS`，并且必须用 `MEW_TRUSTED_PROXY_CIDRS` 限定真实反向代理；反向代理还必须主动覆盖 `X-Real-IP` 和 `X-Forwarded-For`，不得原样透传客户端提交的同名请求头。其他连接提交的转发头会被忽略。

容器使用固定的非 root 用户 `10001:10001`。Linux 或 NAS 在首次启动前必须主动创建可写的数据目录，不能依赖 Docker 自动创建 `./data`，否则该目录通常会属于 `root:root`。从旧版 root 容器升级时，也必须先执行 `docker compose down`，再做一次递归权限迁移：

```bash
chmod 600 ./.env
sudo mkdir -p ./data/assets ./data/.tmp
sudo chown -R 10001:10001 ./data
sudo find ./data -type d -exec chmod 750 {} \;
sudo find ./data -type f -exec chmod 600 {} \;
```

Windows 和 macOS Docker Desktop 的 bind mount 通常不需要手工修改 UID/GID；如果启动日志提示 `/data` 无写入权限，再检查目录共享和 ACL。

如果容器反复重启且日志出现 `Permission denied (os error 13)`，OpenResty/Nginx 的 `502 Bad Gateway` 只是后端未能启动的连带结果。按上面的命令修正实际挂载到 `/data` 的宿主机目录后重新启动即可，不要把应用改回 root 用户。SELinux 主机还需为 bind mount 添加私有标签 `./data:/data:rw,Z`；CIFS、NFS 或 NAS 共享则需要在共享 ACL 或挂载参数中授予 UID/GID `10001` 写权限。

确认配置和目录权限后启动：

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs -f app
```

Compose 默认仅绑定宿主机 `127.0.0.1:3188`，适合宿主机上直接运行的 Nginx 或 Caddy。运行在另一个容器中的 Nginx Proxy Manager 无法直接访问宿主机回环地址；应通过 Compose override 加入共享 Docker 网络并直接代理 `app:3000`，或只向受防火墙保护的地址发布端口。需要直接通过服务器 IP 访问时，把 `.env` 中的 `MEW_HOST_BIND` 显式改为 `0.0.0.0`。SQLite 和登录同步图片统一保存在当前目录的 `./data`：

```text
mewimage/
├── docker-compose.yml
├── .env
└── data/
    ├── mew-image.db
    └── assets/
```

首次打开页面后，注册第一个账号，并在折叠的管理员初始化入口填写 `MEW_ADMIN_TOKEN`。第一个管理员建立后，普通用户注册会进入待审批状态。

说明：

- 直连或宿主机反代部署不需要修改 `docker-compose.yml`；镜像、端口和应用配置全部从同目录 `.env` 读取。容器化 NPM 只需额外使用 Compose override 接入共享网络，无需修改下载的基础文件。
- 默认拉取 `mewlab/mewimage:latest`，可以通过 `.env` 中的 `MEW_DOCKER_IMAGE` 固定具体版本。
- 当前 Docker Hub 镜像提供 `linux/amd64` 架构；ARM64 服务器或 NAS 需要后续发布多架构镜像。
- 默认只在宿主机 `127.0.0.1:3188` 监听；`MEW_HOST_BIND` 和 `MEW_HOST_PORT` 分别控制绑定地址与端口。
- Compose 默认以非 root 用户运行，并启用只读根文件系统、最小 capabilities 和 `no-new-privileges`；持久写入只发生在 `/data`，`/tmp` 是有大小限制的内存临时盘。
- 排队任务的参考图会流式写入 `/data/.tmp/mew-image-proxy`，开始执行后才校验并载入内存；任务完成、取消、超时及后续启动清理都会移除临时文件，API Key 不会写入该目录。
- 镜像内置 `/api/health` 健康检查和 30 秒优雅停止窗口；`docker compose ps` 可查看健康状态。
- 构建阶段会为 WASM、JavaScript、CSS 和 SVG 生成 Brotli/Gzip 预压缩文件，后端会根据浏览器能力自动选择。
- 默认启用 Local 图片存储，不需要额外部署 S3，登录同步图片会写入 `./data/assets`。
- 云端图片默认单文件不超过 64 MiB、每个已审批用户不超过 5 GiB；达到配额后仍可读取、下载和删除已有资源，但不能继续新增。
- 默认设置 `1 GiB` 容器内存上限，可以通过 `MEW_MEMORY_LIMIT` 调整。
- 生成、云端原图上传/下载和游客图片代理共享受控字节预算；上传正文会在鉴权与上传令牌校验后才有界读取，S3 连接和操作也设有明确超时。
- 游客每 IP 默认最多同时运行 4 个生成任务和 2 个结果图代理请求；10 分钟内默认分别允许 30 次和 120 次请求。
- 公网第三方中转站不需要加入白名单，但必须解析到安全公网地址且默认使用 HTTPS；高级 `CustomHttp` 始终需要已审批账号。
- 新部署应使用 `.env.example` 中的 `MEW_GUEST_*_MAX_ACTIVE` 和 `MEW_GUEST_IMAGE_FETCH_RATE_LIMIT`；旧版 `*_CONCURRENCY` 与 `MEW_GUEST_IMAGE_RATE_LIMIT` 名称仍可兼容读取，不建议新旧名称同时配置。
- 新增安全配置同时接受对应的 `MEW_IMAGE_*` 兼容别名，完整映射见 `.env.example`；同一项同时存在时优先使用 `MEW_*` 主名称。
- 生图任务提交后由浏览器短轮询结果，通常不需要为 Nginx Proxy Manager 单独放大数分钟的读取超时。

后端统一发送 CSP、`X-Content-Type-Options`、`Referrer-Policy`、`Permissions-Policy` 和防嵌入响应头。登录会轮换 Session ID；修改密码、账号状态或权限后，旧会话会立即失效。


## 本地开发

1. 安装依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
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
- 若要让本地后端访问同一台电脑上的明文图像上游，请先停止后端，再根据系统在启动后端的同一个终端中设置以下临时环境变量。

Windows PowerShell：

```powershell
$env:MEW_ALLOW_HTTP_UPSTREAM = "true"
$env:MEW_ALLOW_LOOPBACK_UPSTREAM = "true"
cargo run -p mew-image-backend
```

Linux/macOS Bash 或 Zsh：

```bash
export MEW_ALLOW_HTTP_UPSTREAM=true
export MEW_ALLOW_LOOPBACK_UPSTREAM=true
cargo run -p mew-image-backend
```

Linux/macOS 也可以只对本次启动设置变量：

```bash
MEW_ALLOW_HTTP_UPSTREAM=true \
MEW_ALLOW_LOOPBACK_UPSTREAM=true \
cargo run -p mew-image-backend
```

该开发开关只允许显式的 `127.0.0.1`、`::1`、`localhost` 和 `localhost.localdomain`，且仅在 MewImage 后端自身监听环回地址时生效；`10.x`、`172.16–31.x`、`192.168.x` 等局域网地址仍会被拒绝。默认 Docker 监听 `0.0.0.0:3000`，因此该开关不会放宽容器部署。直接执行 `cargo run` 不会自动读取项目根目录的 `.env`，使用上述命令最可靠。测试结束后可关闭当前终端；PowerShell 也可执行 `Remove-Item Env:MEW_ALLOW_LOOPBACK_UPSTREAM, Env:MEW_ALLOW_HTTP_UPSTREAM`，Bash/Zsh 可执行 `unset MEW_ALLOW_LOOPBACK_UPSTREAM MEW_ALLOW_HTTP_UPSTREAM` 清除临时变量。


### 账号规则

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
