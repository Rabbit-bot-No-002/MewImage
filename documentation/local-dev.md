# 本地开发与联调

> 起本地前后端、初始化第一个管理员、让后端访问本机或局域网上游，以及测试时踩过的坑。

本地要起两个进程：后端 `127.0.0.1:3000`，前端 `trunk serve` 在 `8080`。日常用 `http://127.0.0.1:3000` 就能拿到完整功能；只开 `trunk serve` 时纯前端部分可用，涉及后端代理的能力会受限。

## 依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked   # 版本和 Dockerfile 保持一致
```

Linux 上还需要常见的构建工具链（`gcc`、`pkg-config`、`libssl` 开发包等）和 `openssl` 命令，用发行版的包管理器装即可。前后端一起编大概要几百个依赖包。

## 启动

```bash
cargo run -p mew-image-backend     # 后端，默认 127.0.0.1:3000

cd frontend && trunk serve --open  # 前端，8080
```

## 初始化第一个管理员

后端第一次跑起来时数据库是空的，需要自己造管理员。`MEW_AUTH_SECRET` 和 `MEW_ADMIN_TOKEN` 都要用随机值。

**Linux / macOS（bash、zsh）**

```bash
export MEW_AUTH_SECRET="$(openssl rand -hex 32)"
export MEW_ADMIN_TOKEN="$(openssl rand -base64 32)"
export MEW_ALLOW_ADMIN_SETUP=true

# 需要访问明文 HTTP 上游时再加这两行
export MEW_ALLOW_HTTP_UPSTREAM=true
export MEW_DEV_BYPASS_UPSTREAM_SSRF=true

cargo run -p mew-image-backend
```

**Windows（PowerShell）**——和上面等价：

```powershell
# MEW_ADMIN_TOKEN —— 等价 openssl rand -base64 32
$b = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
$env:MEW_ADMIN_TOKEN = [Convert]::ToBase64String($b)

# MEW_AUTH_SECRET —— 等价 openssl rand -hex 32（小写十六进制）
$b = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
$env:MEW_AUTH_SECRET = [System.BitConverter]::ToString($b).Replace('-','').ToLowerInvariant()
```

> ℹ️ **为什么不用 RandomNumberGenerator::Fill**
>
> Windows PowerShell 5.1 跑在 .NET Framework 上，没有 .NET Core 才有的静态方法 `RandomNumberGenerator::Fill`，调用会报 MethodNotFound。用 `Create().GetBytes()` 这条路在两边都能跑。

托管账号要用到 `MEW_MANAGED_PROVIDER_SECRET`（同样用 hex 生成），设好后固定备份——丢了已有的托管配置解不开。

这些变量只在设置它的那个终端里有效，所以**和后端在同一个窗口**里导出。绑定地址也要显式给一下，环回相关的开关才生效：

```bash
export MEW_LISTEN=127.0.0.1:3000        # Windows：$env:MEW_LISTEN = "127.0.0.1:3000"
```

然后在页面上走：**设置 → 账号与同步 → 注册 → 使用管理员初始化口令**，填入刚才输出的 `MEW_ADMIN_TOKEN`。密码至少 10 位，包含大小写字母、数字和符号。这个第一个账号直接是 `approved` 状态的管理员。

如果已经注册过普通账号、但库里还没有管理员，也可以先登录那个账号，再点「将当前账号初始化为管理员」。

检查状态：

```bash
curl http://127.0.0.1:3000/api/auth/setup-status          # Linux / macOS
```

```powershell
Invoke-RestMethod http://127.0.0.1:3000/api/auth/setup-status   # Windows
```

`admin_exists` 为 `true` 表示库里已经有管理员，这条初始化路径不再允许创建第二个。

## `.env` 不会被自动读取

`cargo run` **不会**自动加载项目根目录的 `.env`（那是 `docker compose` 的行为）。要读到变量，只能在启动后端的同一个终端里先设置，或者写在命令前面：

```bash
MEW_LISTEN=127.0.0.1:3000 cargo run -p mew-image-backend
```

在前端终端里设置变量不影响已经启动的后端。

## 允许明文 HTTP 上游

上游是公网 IP、但只提供 HTTP 时，启动后端前加一个开关就够：

```powershell
$env:MEW_ALLOW_HTTP_UPSTREAM = "true"
cargo run -p mew-image-backend
```

改完配置**必须重启后端**，变量只在启动时读一次。

这个开关只放行明文 HTTP 这一件事，**不会**放行下面这些地址：

- `127.0.0.1`、`localhost`
- `10.x.x.x`
- `172.16.x.x` – `172.31.x.x`
- `192.168.x.x`
- 链路本地、CGNAT 和保留地址

所以局域网和本机上游仍然会被拒。这种情况下面两条路更省事：给测试服务套一层 HTTPS 隧道，或者配一个本地域名加证书。管理员登录不会绕过这层限制，它是服务器级策略。

## 绕过上游 SSRF 检查（Fake-IP / 局域网）

本地要完整测这些上游时，除了 HTTP 开关，再加上绕过主机与解析 IP 检查的开关：

```powershell
Set-Location D:\project\MewImage
$env:MEW_LISTEN = "127.0.0.1:3000"
$env:MEW_ALLOW_HTTP_UPSTREAM = "true"
$env:MEW_DEV_BYPASS_UPSTREAM_SSRF = "true"
cargo run -p mew-image-backend
```

```bash
# Linux / macOS 也可以只给这一次启动设变量
MEW_LISTEN=127.0.0.1:3000 \
MEW_ALLOW_HTTP_UPSTREAM=true \
MEW_DEV_BYPASS_UPSTREAM_SSRF=true \
cargo run -p mew-image-backend
```

> ⚠️ **两个绕过开关都要求后端自己监听环回地址**
>
> `MEW_ALLOW_LOOPBACK_UPSTREAM` 只放行显式的 `127.0.0.1`、`::1`、`localhost`、`localhost.localdomain`。`MEW_DEV_BYPASS_UPSTREAM_SSRF` 会跳过生成、图片下载和图片重定向的主机与解析 IP 检查，但绑定 `0.0.0.0`、`[::]` 或局域网地址时会**被忽略并记一条警告**。启动日志会写明开关到底生效没有——先看日志，别猜。这个模式能访问本机和内部服务，不要通过反向代理或隧道对外暴露。

测完关掉终端，或者清掉变量再重启后端：

```powershell
Remove-Item Env:MEW_DEV_BYPASS_UPSTREAM_SSRF, Env:MEW_ALLOW_HTTP_UPSTREAM
```

```bash
unset MEW_DEV_BYPASS_UPSTREAM_SSRF MEW_ALLOW_HTTP_UPSTREAM
```

## 测试时常见的坑

- **生图还会去下载图片 URL**，而且逐次检查重定向。即使 API 域名是公网地址，只要图片链接解析到私网或代理软件的 Fake-IP（如 `198.18.x.x`），仍然会被拦。错误信息里有目标主机，后端日志里有实际解析到的地址，用它定位。
- **允许来源要对得上**。前端跑在 `8080` 时默认的 `MEW_ALLOWED_ORIGINS` 已经包含 `http://127.0.0.1:8080`；换了端口要自己加，末尾不要带 `/`。
- **Windows 下跑整个工作区测试**：`Cargo.toml` 里的 `[profile.test]` 关掉了调试符号，因为完整工作区测试会超过 MSVC 单个 PDB 的容量。别改回来。
- **依赖审计**：`deny.toml` 里挂着两条 Leptos 0.8 间接依赖的 RUSTSEC 例外，带到期时间。升级 Leptos 时要回头清掉。

## 编译成本在哪

`Cargo.lock` 里 514 个包（464 个不同名字），其中一部分是只在 Windows 上编译的 `windows-*`。前后端一起编等于两套编译：前端还要额外编一遍 `wasm32` 目标。按边际包数看，前端 `leptos` 独有 118 个，后端 `sqlx`（只开 sqlite）37 个、`reqwest` 10 个、`tracing-subscriber` 8 个、`image` 8 个。

和日常开发有关的只有两条：

- **`shared` 被前后端同时依赖**，改它两边都要重编。
- 只改后端时 `cargo run -p mew-image-backend` 就够了，不碰前端那一半；`cargo build --workspace` 会把前端 WASM 一起拉进来，`cargo test --workspace` 更重，按 crate 分开跑。

## 相关页面

- 生产部署与镜像：[部署](deploy.md)
- 环境变量逐项说明：[环境变量参考](config.md)
- 账号、审批与登录防护：[账号、审批与登录防护](accounts.md)
- 上游被拦的完整排查：[排障](troubleshooting.md)
---

[← 文档目录](README.md) · [上一页：排障](troubleshooting.md) · [下一页：架构与设计原则](architecture.md)
