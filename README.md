# Lucky WOL Agent

**Lucky WOL Agent** 是一个适配 [Lucky](https://lucky.sirling.com) 网络唤醒功能的第三方轻量受控端，基于 [Tauri v2](https://tauri.app) 构建，运行于 Windows 桌面。

受控端通过 WebSocket 与 Lucky 主控端保持长连接，响应主控端下发的远程指令，包括局域网 WOL 魔术包中继、关机 / 休眠 / 睡眠等操作。

---

## 功能特性

- **实时连接**：WebSocket 长连接 Lucky 主控端，自动重连，连接状态实时显示
- **WOL 中继**：接收主控下发的唤醒请求，通过 UDP 广播魔术包唤醒局域网内其他设备
- **远程执行**：支持接收主控指令执行本机关机 / 休眠 / 睡眠，并提供倒计时确认对话框
- **配置同步**：支持从主控端同步设备配置（设备名、MAC 地址、广播地址等）
- **系统托盘**：最小化至系统托盘后台常驻
- **开机自启**：一键设置 Windows 开机自动启动
- **网卡选择器**：自动枚举本机所有网卡 MAC 地址，点选即填
- **DES 加密协议**：使用与 Lucky 主控端兼容的 DES/ECB 加密通信协议

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 桌面框架 | [Tauri v2](https://tauri.app) |
| 前端 | React 19 + TypeScript + Vite |
| 后端 | Rust |
| UI 图标 | [Lucide React](https://lucide.dev) |
| 加密 | DES/ECB（兼容 Lucky 协议） |
| WebSocket | tokio-tungstenite（支持 TLS） |

---

## 开发环境要求

- [Node.js](https://nodejs.org) >= 18
- [Rust](https://rustup.rs) 稳定版（>= 1.77.2）
- [Tauri CLI v2](https://tauri.app/start/prerequisites/)
- Windows 10 / 11（受控端功能仅支持 Windows）

安装 Tauri CLI：

```bash
cargo install tauri-cli --version "^2"
```

---

## 快速开始

### 克隆项目

```bash
git clone https://github.com/your-username/lucky-wol-agent.git
cd lucky-wol-agent
```

### 安装前端依赖

```bash
npm install
```

### 启动开发模式

```bash
npm run tauri:dev
```

### 构建发布包

```bash
npm run tauri:build
```

构建产物位于 `src-tauri/target/release/bundle/` 目录。

---

## 配置说明

首次启动后，在主界面填写以下配置并点击「保存配置」：

| 配置项 | 说明 |
|--------|------|
| **主控地址** | Lucky 主控端的地址，支持 `ws://`、`wss://`、`http://`、`https://` 或 `host:port` 格式 |
| **Token** | 与 Lucky 主控端中设置的 Token 保持一致 |
| **设备名称** | 显示在 Lucky 主控端界面中的设备标识名 |
| **MAC 地址** | 本机网卡的 MAC 地址，Wake-on-LAN 功能必填；可点击网卡图标从列表中选取 |
| **广播地址** | 当前设备所在局域网的广播地址，例如 `192.168.1.255` |

---

## 与 Lucky 主控端的关系

本项目是 **受控端（Agent）**，不包含主控端功能。使用前需要先在 Lucky 中配置「网络唤醒」功能并获取对应的主控地址和 Token。

- Lucky 官网：https://lucky666.cn/

---

## 项目结构

```
lucky-wol-agent/
├── src/                  # React 前端源码
│   ├── App.tsx           # 主界面组件
│   └── App.css           # 样式
├── src-tauri/            # Rust / Tauri 后端
│   ├── src/
│   │   ├── lib.rs        # 核心逻辑（WebSocket、WOL、加密、命令）
│   │   └── main.rs       # 入口
│   └── Cargo.toml
├── package.json
└── vite.config.ts
```

---

## License

[MIT](./LICENSE)

---

## 免责声明

本项目为第三方开源实现，与 Lucky 官方团队无关。使用前请确认遵守 Lucky 的相关使用条款。
