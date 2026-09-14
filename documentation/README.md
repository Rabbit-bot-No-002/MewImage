# 喵图文档

> Rust 构建的本地优先生图平台。先看这里，找到你需要的部分。

**喵图 MewImage** 是一个用 Rust 写的本地优先图片生成平台：不登录就能完整使用本地工作台，登录并审批通过后再获得跨设备同步与服务器图片存储。

- 前端 `Leptos CSR`（WASM），后端 `Axum + SQLite`，单容器 Docker 部署。
- 历史、会话、参考图、收藏、服务商配置和 API Key 默认只存在你的浏览器里。
- 同步是**手动触发**的增强能力，不是日常使用的前提。

## 我该从哪看起

| 你的情况 | 建议路径 |
| --- | --- |
| 想先跑起来看看 | [快速开始](quickstart.md) → [部署](deploy.md) |
| 已经在部署，卡在某个配置项 | [环境变量参考](config.md) → [排障](troubleshooting.md) |
| 想知道能做什么 | [工作台与生图](workbench.md) → [参考图与编辑器](editor.md) |
| 要给团队/朋友开账号 | [账号与审批](accounts.md) → [托管账号](managed.md) |
| 想弄清数据到底存在哪 | [手动云同步](sync.md) → [后端资源存储](storage.md) |
| 准备读代码或二次开发 | [架构与设计原则](architecture.md) → [后端 API](api.md) |

## 版本

当前文档对应 `v1.1.1`。文档与代码同源维护，若发现与运行中的实例不一致，以实际部署的镜像版本为准。

> ℹ️ **关于术语**
>
> 文档里的「工作台」指生成主界面，「画廊」指结果列表，「广场」指公开模板广场，「托管账号」指由管理员在服务端托管上游连接信息的账号。

## 相关链接

- 源码与 issue：[github.com/Rabbit-bot-No-002/MewImage](https://github.com/Rabbit-bot-No-002/MewImage)
- 镜像：[hub.docker.com/r/mewlab/mewimage](https://hub.docker.com/r/mewlab/mewimage)
- 视频演示：[Bilibili](https://www.bilibili.com/video/BV1njKa6VEm8/) · [YouTube](https://youtu.be/43DXly6Cw5U)
---

[← 文档目录](README.md) · [下一页：快速开始](quickstart.md)
