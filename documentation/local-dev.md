# 本地开发与联调

> 起本地前后端、让后端访问同机或局域网上游、以及测试时容易踩到的几个坑。

本地跑起来要起两个进程：后端 `127.0.0.1:3000`，前端 `trunk serve` 在 `8080`。日常用 `http://127.0.0.1:3000` 就能拿到完整功能；只开 `trunk serve` 时纯前端部分可用，涉及后端代理的能力会受限。

## 依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked   # 版本要和 Dockerfile / CI 对齐
```

## 启动

```bash
cargo run -p mew-image-backend    # 后端，默认 127.0.0.1:3000

cd frontend && trunk serve --open  # 前端，8080
```

## `.env` 不会被自动读取

`cargo run` **不会**自动加载项目根目录的 `.env`——那是 `docker compose` 的行为。想让后端读到某个变量，只能在**启动后端的同一个终端**里先 `export`，或者写在命令前面。

```bash
# Linux / macOS
MEW_LISTEN=127.0.0.1:3000 cargo run -p mew-image-backend

# Windows PowerShell
$env:MEW_LISTEN = "127.0.0.1:3000"
cargo run -p mew-image-backend
```

在前端终端里设置变量不影响已经启动的后端。

## 让后端访问本机或局域网的上游

后端默认拒绝私网、链路本地和环回地址。本地联调时按情况开对应的开关，**都要在启动后端的那个终端里设置**：

| 场景 | 需要的变量 |
| --- | --- |
| 同机的明文 HTTP 上游 | `MEW_ALLOW_HTTP_UPSTREAM=true` + `MEW_ALLOW_LOOPBACK_UPSTREAM=true` |
| 上游解析到局域网或代理软件的 Fake-IP（如 `198.18.x.x`） | `MEW_LISTEN=127.0.0.1:3000` + `MEW_ALLOW_HTTP_UPSTREAM=true` + `MEW_DEV_BYPASS_UPSTREAM_SSRF=true` |

```bash
# 例：完整绕过主机与解析 IP 检查（仅本机开发）
MEW_LISTEN=127.0.0.1:3000 \
MEW_ALLOW_HTTP_UPSTREAM=true \
MEW_DEV_BYPASS_UPSTREAM_SSRF=true \
cargo run -p mew-image-backend
```

> ⚠️ **两个开关都要求后端自己监听环回地址**
>
> `MEW_ALLOW_LOOPBACK_UPSTREAM` 只放行显式的 `127.0.0.1`、`::1`、`localhost`、`localhost.localdomain`；`10.x`、`172.16–31.x`、`192.168.x` 仍然被拒。`MEW_DEV_BYPASS_UPSTREAM_SSRF` 会跳过生成、图片下载和图片重定向的主机与解析 IP 检查，但绑定 `0.0.0.0`、`[::]` 或局域网地址时**会被忽略并记一条警告**。启动日志会写明开关到底生效没有——先看日志，别猜。

## 测试时常见的坑

- **生图还会去下载图片 URL**，而且逐次检查重定向。所以即使 API 域名是公网地址，只要图片链接解析到私网或 Fake-IP，仍然会被拦。错误信息里有目标主机，后端日志里有实际解析到的地址，用它定位。
- **改完环境变量要重启后端**。变量只在启动时读一次，`trunk serve` 的热更新不会带走后端。
- **`MEW_DEV_BYPASS_UPSTREAM_SSRF` 打开后是全局放行**，不要再通过反向代理或隧道把它暴露出去；测完关掉终端或清变量：
  - PowerShell：`Remove-Item Env:MEW_DEV_BYPASS_UPSTREAM_SSRF, Env:MEW_ALLOW_HTTP_UPSTREAM`
  - Bash/Zsh：`unset MEW_DEV_BYPASS_UPSTREAM_SSRF MEW_ALLOW_HTTP_UPSTREAM`
- **允许来源要对得上**。前端跑在 `8080` 时，后端默认的 `MEW_ALLOWED_ORIGINS` 已经包含 `http://127.0.0.1:8080`；换了端口要自己加，末尾不要带 `/`。
- **Windows 下跑整个工作区测试**：`Cargo.toml` 里 `[profile.test]` 关掉了调试符号，原因是完整工作区测试会超过 MSVC 单个 PDB 的容量。别把它改回来。
- **依赖审计**：`deny.toml` 里挂着两条 Leptos 0.8 间接依赖的 RUSTSEC 例外，带到期时间。升级 Leptos 时要回头清掉。

## 相关页面

- 生产构建与镜像：[部署](deploy.md)
- 环境变量逐项说明：[环境变量参考](config.md)
- 上游被拦的完整排查：[排障](troubleshooting.md)
---

[← 文档目录](README.md) · [上一页：排障](troubleshooting.md) · [下一页：架构与设计原则](architecture.md)
