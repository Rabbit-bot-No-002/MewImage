# MewImage

一个使用 Rust 构建的前后端结合图片生成平台：

- 前端：`Leptos CSR`，本地优先，数据保存在浏览器 IndexedDB
- 后端：`Axum + SQLite + S3 兼容对象存储`
- 体验：游客也可直接使用，登录后提供跨设备同步
- 兼容：支持 OpenAI / OpenAI 兼容接口、`nano banana` 预设、自定义 HTTP 模板
- 兜底：内置浏览器直连失败后的同源代理切换，缓解 CORS 问题

## 当前首版能力

- 本地提示词、历史、收藏、参考图、遮罩图、主题偏好持久化
- 账号密码登录
- 云端同步接口
- S3 兼容对象存储资源上传与读取
- OpenAI 兼容接口直连 / 代理生成
- 可爱二次元风格界面与日夜模式切换

## 项目结构

```text
backend/   Axum 后端
frontend/  Leptos CSR 前端
shared/    前后端共享类型与同步/尺寸规则
```

## 本地开发

1. 准备依赖

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
```

2. 配置环境变量

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

## 生产构建

```bash
cargo build -p mew-image-backend --release
cd frontend
trunk build --release --dist dist-app
```

然后让后端通过 `MEW_IMAGE_FRONTEND_DIST` 指向 `frontend/dist-app`。

说明：

- `trunk serve` 默认会使用开发态静态目录，建议不要和后端正在服务的发布目录共用。
- 当前项目默认让后端服务 `frontend/dist-app`，这样就算本地同时开着 `trunk serve`，也不会污染后端页面。

## Docker

```bash
docker compose up --build
```

说明：

- 应用默认监听 `3000`
- MinIO 默认开放 `9000` 与控制台 `9001`
- 首次启动后请在 MinIO 中创建 `MEW_IMAGE_S3_BUCKET` 对应的桶

## 说明

- 当前环境里没有安装 Docker，因此仓库内提供了 Docker 产物，但未在本机完成容器实跑验证
