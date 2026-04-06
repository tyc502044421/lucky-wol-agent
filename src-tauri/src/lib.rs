use std::{
  collections::VecDeque,
  fs,
  net::UdpSocket,
  path::PathBuf,
  process::Command,
  sync::{Arc, Mutex, RwLock},
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Local;
use cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyInit};
use des::Des;
use ecb::{Decryptor, Encryptor};
use futures_util::{SinkExt, StreamExt};
use hostname::get as get_hostname;
use mac_address::get_mac_address;
use native_tls::TlsConnector as NativeTlsConnector;
use serde::{Deserialize, Serialize};
use tauri::{
  async_runtime::JoinHandle,
  menu::{Menu, MenuEvent, MenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
  AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tokio::time::sleep;
use tokio_tungstenite::{
  connect_async,
  connect_async_tls_with_config,
  tungstenite::{self, client::IntoClientRequest, Message},
  Connector,
};
use uuid::Uuid;

const CONFIG_FOLDER: &str = "LuckyWOLAgent";
const CONFIG_FILE: &str = "agent-config.json";
const MAX_EVENTS: usize = 24;

type DesEncryptor = Encryptor<Des>;
type DesDecryptor = Decryptor<Des>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct AppConfig {
  host: String,
  token: String,
  message_key: String,
  skip_cert_verification: bool,
  execution_action: String,
  device_key: String,
  device_name: String,
  mac: String,
  broadcast_ip: String,
  relay: bool,
  wol_port: u16,
  wol_repeat: u8,
  power_off_cmd: String,
  auto_connect: bool,
  minimize_to_tray: bool,
  launch_at_startup: bool,
  update_time: i64,
}

impl Default for AppConfig {
  fn default() -> Self {
    Self {
      host: String::new(),
      token: String::new(),
      message_key: "lucky666".into(),
      skip_cert_verification: false,
      execution_action: "shutdown".into(),
      device_key: format!("Client_{}", Uuid::new_v4().simple()),
      device_name: default_device_name(),
      mac: default_mac_address(),
      broadcast_ip: String::new(),
      relay: true,
      wol_port: 9,
      wol_repeat: 5,
      power_off_cmd: default_poweroff_cmd(),
      auto_connect: true,
      minimize_to_tray: true,
      launch_at_startup: false,
      update_time: unix_now(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionSnapshot {
  phase: ConnectionPhase,
  connected: bool,
  detail: String,
  endpoint: String,
  attempts: u32,
  last_error: Option<String>,
  last_event_at: Option<String>,
}

impl Default for ConnectionSnapshot {
  fn default() -> Self {
    Self {
      phase: ConnectionPhase::Idle,
      connected: false,
      detail: "等待配置".into(),
      endpoint: String::new(),
      attempts: 0,
      last_error: None,
      last_event_at: None,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ConnectionPhase {
  Idle,
  Connecting,
  Connected,
  Reconnecting,
  Disconnected,
  Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityEvent {
  level: EventLevel,
  title: String,
  detail: String,
  timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum EventLevel {
  Info,
  Success,
  Warning,
  Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapPayload {
  config: AppConfig,
  connection: ConnectionSnapshot,
  pending_shutdown: Option<ShutdownPromptPayload>,
  autostart_enabled: bool,
  recent_events: Vec<ActivityEvent>,
  version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownPromptPayload {
  deadline_unix_ms: i64,
  duration_seconds: u32,
  command_preview: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LoginResp {
  #[serde(rename = "Ret")]
  ret: i32,
  #[serde(rename = "Msg")]
  msg: String,
  #[serde(rename = "SystemNowTime", default)]
  system_now_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct SyncClientConfigure {
  #[serde(rename = "Enable")]
  enable: bool,
  #[serde(rename = "ServerURL")]
  server_url: String,
  #[serde(rename = "InsecureSkipVerify", default)]
  insecure_skip_verify: bool,
  #[serde(rename = "Token")]
  token: String,
  #[serde(rename = "Relay")]
  relay: bool,
  #[serde(rename = "Key")]
  key: String,
  #[serde(rename = "DeviceName")]
  device_name: String,
  #[serde(rename = "Mac")]
  mac: String,
  #[serde(rename = "BroadcastIP")]
  broadcast_ip: String,
  #[serde(rename = "Port")]
  port: u16,
  #[serde(rename = "Repeat")]
  repeat: u8,
  #[serde(rename = "PowerOffCMD")]
  power_off_cmd: String,
  #[serde(rename = "UpdateTime")]
  update_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplyWakeUp {
  #[serde(rename = "MacList")]
  mac_list: Vec<String>,
  #[serde(rename = "BroadcastIPs")]
  broadcast_ips: Vec<String>,
  #[serde(rename = "Port")]
  port: u16,
  #[serde(rename = "Repeat")]
  repeat: u8,
}

#[derive(Debug, Clone)]
struct SharedState {
  app: AppHandle,
  config: Arc<RwLock<AppConfig>>,
  connection: Arc<RwLock<ConnectionSnapshot>>,
  events: Arc<Mutex<VecDeque<ActivityEvent>>>,
  task: Arc<Mutex<Option<JoinHandle<()>>>>,
  shutdown_task: Arc<Mutex<Option<JoinHandle<()>>>>,
  pending_shutdown: Arc<RwLock<Option<ShutdownPromptPayload>>>,
}

impl SharedState {
  fn new(app: AppHandle, config: AppConfig) -> Self {
    Self {
      app,
      config: Arc::new(RwLock::new(config)),
      connection: Arc::new(RwLock::new(ConnectionSnapshot::default())),
      events: Arc::new(Mutex::new(VecDeque::new())),
      task: Arc::new(Mutex::new(None)),
      shutdown_task: Arc::new(Mutex::new(None)),
      pending_shutdown: Arc::new(RwLock::new(None)),
    }
  }

  fn config_snapshot(&self) -> AppConfig {
    self.config.read().expect("config lock poisoned").clone()
  }

  fn set_config(&self, next: AppConfig) -> Result<(), String> {
    persist_config(&next)?;
    *self.config.write().expect("config lock poisoned") = next;
    Ok(())
  }

  fn connection_snapshot(&self) -> ConnectionSnapshot {
    self.connection.read().expect("connection lock poisoned").clone()
  }

  fn set_connection(&self, snapshot: ConnectionSnapshot) {
    *self.connection.write().expect("connection lock poisoned") = snapshot.clone();
    let _ = self.app.emit("agent://connection-updated", snapshot);
  }

  fn patch_connection<F>(&self, mutator: F)
  where
    F: FnOnce(&mut ConnectionSnapshot),
  {
    let next = {
      let mut current = self.connection.write().expect("connection lock poisoned");
      mutator(&mut current);
      current.clone()
    };
    let _ = self.app.emit("agent://connection-updated", next);
  }

  fn push_event(&self, level: EventLevel, title: impl Into<String>, detail: impl Into<String>) {
    let event = ActivityEvent {
      level,
      title: title.into(),
      detail: detail.into(),
      timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };

    {
      let mut queue = self.events.lock().expect("events lock poisoned");
      queue.push_front(event.clone());
      while queue.len() > MAX_EVENTS {
        queue.pop_back();
      }
    }

    self.patch_connection(|snapshot| {
      snapshot.last_event_at = Some(event.timestamp.clone());
    });

    let _ = self.app.emit("agent://activity-logged", event);
  }

  fn recent_events(&self) -> Vec<ActivityEvent> {
    self.events.lock().expect("events lock poisoned").iter().cloned().collect()
  }

  fn bootstrap_payload(&self) -> BootstrapPayload {
    BootstrapPayload {
      config: self.config_snapshot(),
      connection: self.connection_snapshot(),
      pending_shutdown: self.pending_shutdown.read().expect("shutdown prompt lock poisoned").clone(),
      autostart_enabled: self.app.autolaunch().is_enabled().unwrap_or(false),
      recent_events: self.recent_events(),
      version: env!("CARGO_PKG_VERSION").into(),
    }
  }

  fn abort_task(&self) {
    if let Some(handle) = self.task.lock().expect("task lock poisoned").take() {
      handle.abort();
    }
  }

  fn clear_shutdown_prompt(&self) {
    if let Some(handle) = self.shutdown_task.lock().expect("shutdown task lock poisoned").take() {
      handle.abort();
    }
    *self.pending_shutdown.write().expect("shutdown prompt lock poisoned") = None;
    let _ = self.app.emit("agent://shutdown-cleared", ());
  }

  fn schedule_shutdown_prompt(&self, duration_seconds: u32) {
    self.clear_shutdown_prompt();

    let payload = ShutdownPromptPayload {
      deadline_unix_ms: unix_now_ms() + i64::from(duration_seconds) * 1000,
      duration_seconds,
      command_preview: self.config_snapshot().power_off_cmd,
    };
    *self.pending_shutdown.write().expect("shutdown prompt lock poisoned") = Some(payload.clone());
    let _ = self.app.emit("agent://shutdown-pending", payload);
    show_main_window(&self.app);

    let runtime = self.clone();
    let handle = tauri::async_runtime::spawn(async move {
      sleep(Duration::from_secs(u64::from(duration_seconds))).await;
      runtime.clear_shutdown_prompt();
      match execute_poweroff(&runtime.config_snapshot().power_off_cmd) {
        Ok(()) => runtime.push_event(EventLevel::Warning, "收到关机指令", "30 秒倒计时结束，已执行本机关机命令。"),
        Err(error) => runtime.push_event(EventLevel::Error, "关机命令执行失败", error),
      }
    });
    *self.shutdown_task.lock().expect("shutdown task lock poisoned") = Some(handle);
  }

  fn start_connector(&self) {
    self.abort_task();
    let runtime = self.clone();
    let handle = tauri::async_runtime::spawn(async move {
      run_connector_v2(runtime).await;
    });
    *self.task.lock().expect("task lock poisoned") = Some(handle);
  }

  fn stop_connector(&self, detail: &str) {
    self.abort_task();
    self.clear_shutdown_prompt();
    self.set_connection(ConnectionSnapshot {
      phase: ConnectionPhase::Disconnected,
      connected: false,
      detail: detail.into(),
      endpoint: normalize_ws_url(&self.config_snapshot().host),
      attempts: self.connection_snapshot().attempts,
      last_error: None,
      last_event_at: self.connection_snapshot().last_event_at,
    });
    self.push_event(EventLevel::Info, "连接器已停止", detail);
  }
}

#[tauri::command]
fn get_bootstrap(state: State<'_, SharedState>) -> BootstrapPayload {
  state.bootstrap_payload()
}

#[tauri::command]
fn save_config(state: State<'_, SharedState>, config: AppConfig) -> Result<BootstrapPayload, String> {
  let mut next = config;
  next.host = next.host.trim().into();
  next.token = next.token.trim().into();
  next.message_key = "lucky666".into();
  next.execution_action = normalize_execution_action(&next.execution_action);
  next.device_key = if next.device_key.trim().is_empty() {
    format!("Client_{}", Uuid::new_v4().simple())
  } else {
    next.device_key.trim().into()
  };
  next.device_name = next.device_name.trim().into();
  next.mac = next.mac.trim().into();
  next.broadcast_ip = next.broadcast_ip.trim().into();
  next.wol_port = 9;
  next.wol_repeat = 5;
  next.power_off_cmd = power_command_for_action(&next.execution_action);
  next.update_time = unix_now();

  state.set_config(next.clone())?;
  state.push_event(EventLevel::Success, "配置已保存", "新的运行配置已经写入本地。");

  if next.auto_connect && !next.host.is_empty() && !next.token.is_empty() {
    state.start_connector();
  }

  Ok(state.bootstrap_payload())
}

#[tauri::command]
fn connect_now(state: State<'_, SharedState>) -> Result<(), String> {
  let config = state.config_snapshot();
  if config.host.is_empty() || config.token.is_empty() {
    return Err("请先填写主控地址和 Token。".into());
  }

  state.push_event(EventLevel::Info, "手动连接", "正在立即发起主控连接。");
  state.start_connector();
  Ok(())
}

#[tauri::command]
fn reconnect_now(state: State<'_, SharedState>) {
  state.push_event(EventLevel::Info, "请求重连", "正在刷新与主控端的连接。");
  state.start_connector();
}

#[tauri::command]
fn disconnect_now(state: State<'_, SharedState>) {
  state.stop_connector("Stopped by user");
}

#[tauri::command]
fn cancel_pending_shutdown(state: State<'_, SharedState>) {
  state.clear_shutdown_prompt();
  state.push_event(EventLevel::Info, "已取消关机", "用户取消了这次关机倒计时。");
}

#[tauri::command]
fn execute_pending_shutdown_now(state: State<'_, SharedState>) -> Result<(), String> {
  state.clear_shutdown_prompt();
  execute_poweroff(&state.config_snapshot().power_off_cmd)?;
  state.push_event(EventLevel::Warning, "立即关机", "用户确认立即执行本机关机命令。");
  Ok(())
}

#[tauri::command]
fn set_launch_at_startup(app: AppHandle, state: State<'_, SharedState>, enabled: bool) -> Result<bool, String> {
  let manager = app.autolaunch();
  if enabled {
    manager.enable().map_err(|error| error.to_string())?;
  } else {
    manager.disable().map_err(|error| error.to_string())?;
  }
  let current = manager.is_enabled().map_err(|error| error.to_string())?;
  let mut config = state.config_snapshot();
  config.launch_at_startup = current;
  state.set_config(config)?;
  state.push_event(
    EventLevel::Info,
    "开机自启已更新",
    if current { "程序将随 Windows 一起启动。" } else { "已关闭开机自启。" },
  );
  Ok(current)
}

async fn run_connector(state: SharedState) {
  let mut attempts = 0u32;

  loop {
    let config = state.config_snapshot();
    let endpoint = normalize_ws_url(&config.host);

    if config.host.is_empty() || config.token.is_empty() {
      state.set_connection(ConnectionSnapshot {
        phase: ConnectionPhase::Idle,
        connected: false,
        detail: "请先填写主控地址和 Token".into(),
        endpoint,
        attempts,
        last_error: None,
        last_event_at: state.connection_snapshot().last_event_at,
      });
      return;
    }

    attempts = attempts.saturating_add(1);
    state.set_connection(ConnectionSnapshot {
      phase: if attempts == 1 { ConnectionPhase::Connecting } else { ConnectionPhase::Reconnecting },
      connected: false,
      detail: "正在连接 Lucky 主控端".into(),
      endpoint: endpoint.clone(),
      attempts,
      last_error: None,
      last_event_at: state.connection_snapshot().last_event_at,
    });

    let connect_target = match resolve_websocket_endpoint(&endpoint, &config).await {
      Ok(target) => target,
      Err(error) => {
        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Error,
          connected: false,
          detail: "无法连接到主控端".into(),
          endpoint: endpoint.clone(),
          attempts,
          last_error: Some(error.clone()),
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Error, "连接失败", error);
        sleep(Duration::from_secs(3)).await;
        continue;
      }
    };

    match connect_with_config(&config, &connect_target).await {
      Ok((stream, _)) => {
        state.push_event(EventLevel::Success, "WebSocket 已连接", format!("已连接到 {connect_target}，等待 Lucky 登录响应。"));
        let (mut write, mut read) = stream.split();

        match create_login_message(&config) {
          Ok(payload) => {
            state.push_event(
              EventLevel::Info,
              "准备发送 Login",
              format!(
                "deviceKey={} mac={} relay={} updateTime={}",
                config.device_key, config.mac, config.relay, config.update_time
              ),
            );
            if let Err(error) = write.send(Message::Text(payload.into())).await {
              state.patch_connection(|snapshot| {
                snapshot.phase = ConnectionPhase::Error;
                snapshot.connected = false;
                snapshot.detail = "登录消息发送失败".into();
                snapshot.endpoint = connect_target.clone();
                snapshot.attempts = attempts;
                snapshot.last_error = Some(error.to_string());
              });
              state.push_event(EventLevel::Error, "登录发送失败", error.to_string());
              sleep(Duration::from_secs(3)).await;
              continue;
            }
            state.push_event(EventLevel::Info, "Login 已发送", "已向 Lucky 主控端发送登录消息，等待响应。");
          }
          Err(error) => {
            state.push_event(EventLevel::Error, "协议封包失败", error.clone());
            state.patch_connection(|snapshot| {
              snapshot.phase = ConnectionPhase::Error;
              snapshot.connected = false;
              snapshot.detail = "协议消息构造失败".into();
              snapshot.endpoint = connect_target.clone();
              snapshot.attempts = attempts;
              snapshot.last_error = Some(error);
            });
            sleep(Duration::from_secs(3)).await;
            continue;
          }
        }

        let mut authenticated = false;
        let login_deadline = tokio::time::Instant::now() + Duration::from_secs(8);

        while let Some(message) = read.next().await {
          match message {
            Ok(Message::Text(text)) => {
              let preview: String = text.chars().take(96).collect();
              state.push_event(
                EventLevel::Info,
                "收到入站文本消息",
                format!("长度={}，预览={}", text.len(), preview),
              );

              match unpack_message(text.as_str(), &config.message_key) {
              Ok(IncomingMessage::LoginResp(response)) => {
                state.push_event(
                  EventLevel::Info,
                  "收到 LoginResp",
                  format!(
                    "ret={} msg={} systemNowTime={}",
                    response.ret, response.msg, response.system_now_time
                  ),
                );
                if response.ret == 0 {
                  authenticated = true;
                  state.set_connection(ConnectionSnapshot {
                    phase: ConnectionPhase::Connected,
                    connected: true,
                    detail: "主控连接正常".into(),
                    endpoint: connect_target.clone(),
                    attempts,
                    last_error: None,
                    last_event_at: state.connection_snapshot().last_event_at,
                  });
                  state.push_event(EventLevel::Success, "鉴权成功", "Lucky 主控端已接受当前设备。");
                } else {
                  state.set_connection(ConnectionSnapshot {
                    phase: ConnectionPhase::Error,
                    connected: false,
                    detail: "主控端拒绝登录".into(),
                    endpoint: connect_target.clone(),
                    attempts,
                    last_error: Some(response.msg.clone()),
                    last_event_at: state.connection_snapshot().last_event_at,
                  });
                  state.push_event(EventLevel::Error, "登录被拒绝", response.msg);
                  break;
                }
              }
              Ok(IncomingMessage::SyncClientConfigure(sync)) => {
                state.push_event(
                  EventLevel::Info,
                  "收到配置同步",
                  format!("deviceName={} mac={} broadcastIp={}", sync.device_name, sync.mac, sync.broadcast_ip),
                );
                let mut next = state.config_snapshot();
                next.auto_connect = sync.enable;
                if !sync.server_url.trim().is_empty() {
                  next.host = sync.server_url;
                }
                next.skip_cert_verification = sync.insecure_skip_verify;
                if !sync.token.trim().is_empty() {
                  next.token = sync.token;
                }
                next.relay = sync.relay;
                if !sync.key.trim().is_empty() {
                  next.device_key = sync.key;
                }
                if !sync.device_name.trim().is_empty() {
                  next.device_name = sync.device_name;
                }
                if !sync.mac.trim().is_empty() {
                  next.mac = sync.mac;
                }
                if !sync.broadcast_ip.trim().is_empty() {
                  next.broadcast_ip = sync.broadcast_ip;
                }
                next.wol_port = sync.port;
                next.wol_repeat = sync.repeat;
                if !sync.power_off_cmd.trim().is_empty() {
                  next.power_off_cmd = sync.power_off_cmd;
                }
                next.update_time = sync.update_time;

                if let Err(error) = state.set_config(next) {
                  state.push_event(EventLevel::Warning, "配置同步警告", error);
                } else {
                  state.push_event(EventLevel::Info, "配置已同步", "主控端配置变更已应用到本地。");
                }
              }
              Ok(IncomingMessage::ReplyWakeUp(payload)) => match relay_magic_packets(payload) {
                Ok(()) => state.push_event(EventLevel::Success, "已发送唤醒中继", "魔术包已经在当前局域网内广播。"),
                Err(error) => state.push_event(EventLevel::Error, "唤醒中继失败", error),
              },
              Ok(IncomingMessage::ShutDown) => match execute_poweroff(&state.config_snapshot().power_off_cmd) {
                Ok(()) => state.push_event(EventLevel::Warning, "收到关机指令", "本机关机命令已经执行。"),
                Err(error) => state.push_event(EventLevel::Error, "关机命令执行失败", error),
              },
              Err(error) => state.push_event(EventLevel::Warning, "入站消息解析失败", error),
            }
            },
            Ok(Message::Binary(_)) => {
              state.push_event(EventLevel::Warning, "收到不支持的帧", "当前实现期望接收文本帧，但收到了二进制帧。");
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
              state.set_connection(ConnectionSnapshot {
                phase: ConnectionPhase::Error,
                connected: false,
                detail: "读取连接数据失败".into(),
                endpoint: connect_target.clone(),
                attempts,
                last_error: Some(error.to_string()),
                last_event_at: state.connection_snapshot().last_event_at,
              });
              state.push_event(EventLevel::Error, "主控连接流异常", error.to_string());
              break;
            }
          }

          if !authenticated && tokio::time::Instant::now() > login_deadline {
            state.push_event(
              EventLevel::Warning,
              "登录响应超时",
              "WebSocket 已连接，但在 8 秒内未收到 Lucky LoginResp。",
            );
            state.set_connection(ConnectionSnapshot {
              phase: ConnectionPhase::Error,
              connected: false,
              detail: "已连接主控端，但未收到登录响应".into(),
              endpoint: connect_target.clone(),
              attempts,
              last_error: Some("LoginResp timeout".into()),
              last_event_at: state.connection_snapshot().last_event_at,
            });
            break;
          }
        }

        if !authenticated {
          sleep(Duration::from_secs(4)).await;
          continue;
        }

        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Disconnected,
          connected: false,
          detail: "连接已断开，稍后自动重试".into(),
          endpoint: connect_target.clone(),
          attempts,
          last_error: None,
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Warning, "连接已关闭", "后台连接器将在稍后自动重试。");
        sleep(Duration::from_secs(3)).await;
      }
      Err(error) => {
        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Error,
          connected: false,
          detail: "无法连接到主控端".into(),
          endpoint: connect_target.clone(),
          attempts,
          last_error: Some(error.to_string()),
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Error, "连接失败", error.to_string());
        sleep(Duration::from_secs(3)).await;
      }
    }
  }
}

async fn resolve_websocket_endpoint(endpoint: &str, config: &AppConfig) -> Result<String, String> {
  match connect_with_config(config, endpoint).await {
    Ok((stream, _)) => {
      drop(stream);
      Ok(endpoint.to_string())
    }
    Err(tungstenite::Error::Http(response)) => {
      let status = response.status();
      if !matches!(status.as_u16(), 301 | 302 | 307 | 308) {
        return Err(format!("HTTP error: {status}"));
      }

      let Some(location) = response.headers().get("location") else {
        return Err(format!("HTTP error: {status}，但响应里没有 Location"));
      };

      let location = location
        .to_str()
        .map_err(|error| format!("跳转地址解析失败: {error}"))?;

      Ok(normalize_redirect_target(endpoint, location))
    }
    Err(error) => Err(error.to_string()),
  }
}

async fn run_connector_v2(state: SharedState) {
  let mut attempts = 0u32;

  loop {
    let config = state.config_snapshot();
    let endpoint = normalize_ws_url(&config.host);

    if config.host.is_empty() || config.token.is_empty() {
      state.set_connection(ConnectionSnapshot {
        phase: ConnectionPhase::Idle,
        connected: false,
        detail: "请先填写主控地址和 Token".into(),
        endpoint,
        attempts,
        last_error: None,
        last_event_at: state.connection_snapshot().last_event_at,
      });
      return;
    }

    attempts = attempts.saturating_add(1);
    state.set_connection(ConnectionSnapshot {
      phase: if attempts == 1 {
        ConnectionPhase::Connecting
      } else {
        ConnectionPhase::Reconnecting
      },
      connected: false,
      detail: "正在连接 Lucky 主控端".into(),
      endpoint: endpoint.clone(),
      attempts,
      last_error: None,
      last_event_at: state.connection_snapshot().last_event_at,
    });

    let connect_target = match resolve_websocket_endpoint(&endpoint, &config).await {
      Ok(target) => target,
      Err(error) => {
        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Error,
          connected: false,
          detail: "无法连接到主控端".into(),
          endpoint: endpoint.clone(),
          attempts,
          last_error: Some(error.clone()),
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Error, "连接失败", error);
        sleep(Duration::from_secs(3)).await;
        continue;
      }
    };

    match connect_with_config(&config, &connect_target).await {
      Ok((stream, _)) => {
        state.push_event(
          EventLevel::Success,
          "WebSocket 已连接",
          format!("已连接到 {connect_target}，等待 Lucky 登录响应。"),
        );
        let (mut write, mut read) = stream.split();

        let payload = match create_login_message(&config) {
          Ok(payload) => payload,
          Err(error) => {
            state.push_event(EventLevel::Error, "协议封包失败", error.clone());
            state.patch_connection(|snapshot| {
              snapshot.phase = ConnectionPhase::Error;
              snapshot.connected = false;
              snapshot.detail = "协议消息构造失败".into();
              snapshot.endpoint = connect_target.clone();
              snapshot.attempts = attempts;
              snapshot.last_error = Some(error);
            });
            sleep(Duration::from_secs(3)).await;
            continue;
          }
        };

        state.push_event(
          EventLevel::Info,
          "准备发送 Login",
          format!(
            "deviceKey={} mac={} relay={} updateTime={}",
            config.device_key, config.mac, config.relay, config.update_time
          ),
        );

        if let Err(error) = write.send(Message::Text(payload.into())).await {
          state.patch_connection(|snapshot| {
            snapshot.phase = ConnectionPhase::Error;
            snapshot.connected = false;
            snapshot.detail = "登录消息发送失败".into();
            snapshot.endpoint = connect_target.clone();
            snapshot.attempts = attempts;
            snapshot.last_error = Some(error.to_string());
          });
          state.push_event(EventLevel::Error, "登录发送失败", error.to_string());
          sleep(Duration::from_secs(3)).await;
          continue;
        }

        state.push_event(EventLevel::Info, "Login 已发送", "已向 Lucky 主控端发送登录消息，等待响应。");

        let mut authenticated = false;
        let login_deadline = tokio::time::Instant::now() + Duration::from_secs(8);

        while let Some(message) = read.next().await {
          match message {
            Ok(Message::Text(text)) => {
              let preview: String = text.chars().take(96).collect();
              state.push_event(
                EventLevel::Info,
                "收到入站文本消息",
                format!("长度={}，预览={}", text.len(), preview),
              );

              match unpack_message(text.as_str(), &config.message_key) {
                Ok(IncomingMessage::LoginResp(response)) => {
                  state.push_event(
                    EventLevel::Info,
                    "收到 LoginResp",
                    format!(
                      "ret={} msg={} systemNowTime={}",
                      response.ret, response.msg, response.system_now_time
                    ),
                  );
                  if response.ret == 0 {
                    authenticated = true;
                    state.set_connection(ConnectionSnapshot {
                      phase: ConnectionPhase::Connected,
                      connected: true,
                      detail: "主控连接正常".into(),
                      endpoint: connect_target.clone(),
                      attempts,
                      last_error: None,
                      last_event_at: state.connection_snapshot().last_event_at,
                    });
                    state.push_event(EventLevel::Success, "鉴权成功", "Lucky 主控端已接受当前设备。");
                  } else {
                    state.set_connection(ConnectionSnapshot {
                      phase: ConnectionPhase::Error,
                      connected: false,
                      detail: "主控端拒绝登录".into(),
                      endpoint: connect_target.clone(),
                      attempts,
                      last_error: Some(response.msg.clone()),
                      last_event_at: state.connection_snapshot().last_event_at,
                    });
                    state.push_event(EventLevel::Error, "登录被拒绝", response.msg);
                    break;
                  }
                }
                Ok(IncomingMessage::SyncClientConfigure(sync)) => {
                  state.push_event(
                    EventLevel::Info,
                    "收到配置同步",
                    format!("deviceName={} mac={} broadcastIp={}", sync.device_name, sync.mac, sync.broadcast_ip),
                  );
                  let mut next = state.config_snapshot();
                  next.auto_connect = sync.enable;
                  if !sync.server_url.trim().is_empty() {
                    next.host = sync.server_url;
                  }
                  next.skip_cert_verification = sync.insecure_skip_verify;
                  if !sync.token.trim().is_empty() {
                    next.token = sync.token;
                  }
                  next.relay = sync.relay;
                  if !sync.key.trim().is_empty() {
                    next.device_key = sync.key;
                  }
                  if !sync.device_name.trim().is_empty() {
                    next.device_name = sync.device_name;
                  }
                  if !sync.mac.trim().is_empty() {
                    next.mac = sync.mac;
                  }
                  if !sync.broadcast_ip.trim().is_empty() {
                    next.broadcast_ip = sync.broadcast_ip;
                  }
                  next.wol_port = sync.port;
                  next.wol_repeat = sync.repeat;
                  if !sync.power_off_cmd.trim().is_empty() {
                    next.power_off_cmd = sync.power_off_cmd;
                  }
                  next.update_time = sync.update_time;

                  if let Err(error) = state.set_config(next) {
                    state.push_event(EventLevel::Warning, "配置同步警告", error);
                  } else {
                    state.push_event(EventLevel::Info, "配置已同步", "主控端配置变更已应用到本地。");
                  }
                }
                Ok(IncomingMessage::ReplyWakeUp(payload)) => match relay_magic_packets(payload) {
                  Ok(()) => state.push_event(EventLevel::Success, "已发送唤醒中继", "魔术包已经在当前局域网内广播。"),
                  Err(error) => state.push_event(EventLevel::Error, "唤醒中继失败", error),
                },
                Ok(IncomingMessage::ShutDown) => {
                  state.schedule_shutdown_prompt(30);
                  state.push_event(
                    EventLevel::Warning,
                    "收到关机指令",
                    "主控端请求 30 秒后关机，用户可以取消或立即执行。",
                  );
                }
                Err(error) => state.push_event(EventLevel::Warning, "入站消息解析失败", error),
              }
            }
            Ok(Message::Binary(_)) => {
              state.push_event(EventLevel::Warning, "收到不支持的帧", "当前实现期望接收文本帧，但收到了二进制帧。");
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
              state.set_connection(ConnectionSnapshot {
                phase: ConnectionPhase::Error,
                connected: false,
                detail: "读取连接数据失败".into(),
                endpoint: connect_target.clone(),
                attempts,
                last_error: Some(error.to_string()),
                last_event_at: state.connection_snapshot().last_event_at,
              });
              state.push_event(EventLevel::Error, "主控连接流异常", error.to_string());
              break;
            }
          }

          if !authenticated && tokio::time::Instant::now() > login_deadline {
            state.push_event(
              EventLevel::Warning,
              "登录响应超时",
              "WebSocket 已连接，但在 8 秒内未收到 Lucky LoginResp。",
            );
            state.set_connection(ConnectionSnapshot {
              phase: ConnectionPhase::Error,
              connected: false,
              detail: "已连接主控端，但未收到登录响应".into(),
              endpoint: connect_target.clone(),
              attempts,
              last_error: Some("LoginResp timeout".into()),
              last_event_at: state.connection_snapshot().last_event_at,
            });
            break;
          }
        }

        if !authenticated {
          sleep(Duration::from_secs(4)).await;
          continue;
        }

        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Disconnected,
          connected: false,
          detail: "连接已断开，稍后自动重试".into(),
          endpoint: connect_target.clone(),
          attempts,
          last_error: None,
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Warning, "连接已关闭", "后台连接器将在稍后自动重试。");
        sleep(Duration::from_secs(3)).await;
      }
      Err(error) => {
        state.set_connection(ConnectionSnapshot {
          phase: ConnectionPhase::Error,
          connected: false,
          detail: "无法连接到主控端".into(),
          endpoint: connect_target.clone(),
          attempts,
          last_error: Some(error.to_string()),
          last_event_at: state.connection_snapshot().last_event_at,
        });
        state.push_event(EventLevel::Error, "连接失败", error.to_string());
        sleep(Duration::from_secs(3)).await;
      }
    }
  }
}

fn normalize_redirect_target(current_endpoint: &str, location: &str) -> String {
  let trimmed = location.trim();

  if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
    return trimmed.to_string();
  }

  if trimmed.starts_with("http://") {
    return trimmed.replacen("http://", "ws://", 1);
  }

  if trimmed.starts_with("https://") {
    return trimmed.replacen("https://", "wss://", 1);
  }

  if trimmed.starts_with('/') {
    let base = normalize_controller_base(current_endpoint);
    return format!("{base}{trimmed}");
  }

  let base = normalize_controller_base(current_endpoint);
  format!("{base}/{trimmed}")
}

async fn connect_with_config(
  config: &AppConfig,
  endpoint: &str,
) -> Result<
  (
    tokio_tungstenite::WebSocketStream<
      tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    tungstenite::handshake::client::Response,
  ),
  tungstenite::Error,
> {
  if endpoint.starts_with("wss://") {
    let mut builder = NativeTlsConnector::builder();
    if config.skip_cert_verification {
      builder.danger_accept_invalid_certs(true);
      builder.danger_accept_invalid_hostnames(true);
    }
    let connector = builder
      .build()
      .map_err(|error| tungstenite::Error::Io(std::io::Error::other(error.to_string())))?;
    let connector = Connector::NativeTls(connector);
    let request = endpoint.into_client_request()?;
    connect_async_tls_with_config(request, None, false, Some(connector)).await
  } else {
    connect_async(endpoint).await
  }
}

enum IncomingMessage {
  LoginResp(LoginResp),
  SyncClientConfigure(SyncClientConfigure),
  ReplyWakeUp(ReplyWakeUp),
  ShutDown,
}

fn create_login_message(config: &AppConfig) -> Result<String, String> {
  #[derive(Serialize)]
  struct LoginMessage<'a> {
    #[serde(rename = "Enable")]
    enable: bool,
    #[serde(rename = "ServerURL")]
    server_url: &'a str,
    #[serde(rename = "InsecureSkipVerify")]
    insecure_skip_verify: bool,
    #[serde(rename = "Token")]
    token: &'a str,
    #[serde(rename = "Relay")]
    relay: bool,
    #[serde(rename = "Key")]
    key: &'a str,
    #[serde(rename = "DeviceName")]
    device_name: &'a str,
    #[serde(rename = "Mac")]
    mac: &'a str,
    #[serde(rename = "BroadcastIP")]
    broadcast_ip: &'a str,
    #[serde(rename = "Port")]
    port: u16,
    #[serde(rename = "Repeat")]
    repeat: u8,
    #[serde(rename = "PowerOffCMD")]
    power_off_cmd: &'a str,
    #[serde(rename = "UpdateTime")]
    update_time: i64,
    #[serde(rename = "ClientTimeStamp")]
    client_time_stamp: i64,
  }

  let server_url = normalize_ws_url(&config.host);
  let payload = LoginMessage {
    enable: true,
    server_url: &server_url,
    insecure_skip_verify: config.skip_cert_verification,
    token: &config.token,
    relay: config.relay,
    key: &config.device_key,
    device_name: &config.device_name,
    mac: &config.mac,
    broadcast_ip: &config.broadcast_ip,
    port: config.wol_port,
    repeat: config.wol_repeat,
    power_off_cmd: &config.power_off_cmd,
    update_time: config.update_time,
    client_time_stamp: unix_now(),
  };

  pack_message(b'0', &payload, &config.message_key)
}

fn pack_message<T: Serialize>(message_type: u8, payload: &T, key: &str) -> Result<String, String> {
  let json = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
  let mut buffer = Vec::with_capacity(json.len() + 9);
  buffer.push(message_type);
  buffer.extend_from_slice(&(json.len() as i64).to_be_bytes());
  buffer.extend_from_slice(&json);
  Ok(BASE64.encode(encrypt_message(&buffer, key)?))
}

fn unpack_message(raw_text: &str, key: &str) -> Result<IncomingMessage, String> {
  let encrypted = BASE64.decode(raw_text.as_bytes()).map_err(|error| error.to_string())?;
  let decrypted = decrypt_message(&encrypted, key)?;

  if decrypted.len() <= 9 {
    return Err("解密后的消息长度过短".into());
  }

  let message_type = decrypted[0];
  let payload_len =
    i64::from_be_bytes(decrypted[1..9].try_into().map_err(|_| "json length invalid".to_string())?);
  if payload_len < 0 {
    return Err("json length must not be negative".into());
  }
  let payload_len = payload_len as usize;
  if decrypted.len() < 9 + payload_len {
    return Err(format!(
      "decrypted payload too short: declared json length {}, actual {}",
      payload_len,
      decrypted.len().saturating_sub(9)
    ));
  }
  let payload = &decrypted[9..9 + payload_len];

  match message_type {
    b'1' => serde_json::from_slice::<LoginResp>(payload)
      .map(IncomingMessage::LoginResp)
      .map_err(|error| error.to_string()),
    b'2' => serde_json::from_slice::<SyncClientConfigure>(payload)
      .map(IncomingMessage::SyncClientConfigure)
      .map_err(|error| error.to_string()),
    b'3' => serde_json::from_slice::<ReplyWakeUp>(payload)
      .map(IncomingMessage::ReplyWakeUp)
      .map_err(|error| error.to_string()),
    b'4' => Ok(IncomingMessage::ShutDown),
    other => Err(format!("未知的 Lucky 消息类型: {other}")),
  }
}

fn encrypt_message(plain: &[u8], key: &str) -> Result<Vec<u8>, String> {
  let normalized = normalize_des_key(key);
  let mut padded = plain.to_vec();
  let remainder = padded.len() % 8;
  if remainder != 0 {
    padded.resize(padded.len() + (8 - remainder), 0);
  }
  Ok(DesEncryptor::new((&normalized).into()).encrypt_padded_vec_mut::<NoPadding>(&padded))
}

fn decrypt_message(cipher_text: &[u8], key: &str) -> Result<Vec<u8>, String> {
  if cipher_text.len() % 8 != 0 {
    return Err("DES cipher text length must be aligned to 8 bytes".into());
  }
  let normalized = normalize_des_key(key);
  DesDecryptor::new((&normalized).into())
    .decrypt_padded_vec_mut::<NoPadding>(cipher_text)
    .map_err(|error| error.to_string())
}

fn normalize_des_key(raw: &str) -> [u8; 8] {
  let mut key = [0u8; 8];
  for (index, byte) in raw.as_bytes().iter().take(8).enumerate() {
    key[index] = *byte;
  }
  key
}

fn relay_magic_packets(payload: ReplyWakeUp) -> Result<(), String> {
  let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| error.to_string())?;
  socket.set_broadcast(true).map_err(|error| error.to_string())?;

  for broadcast in payload.broadcast_ips {
    for mac in &payload.mac_list {
      let packet = build_magic_packet(mac)?;
      let target = format!("{broadcast}:{}", payload.port);
      for _ in 0..payload.repeat.max(1) {
        socket.send_to(&packet, &target).map_err(|error| error.to_string())?;
      }
    }
  }

  Ok(())
}

fn build_magic_packet(mac: &str) -> Result<Vec<u8>, String> {
  let cleaned = mac.replace([':', '-'], "");
  if cleaned.len() != 12 {
    return Err(format!("Invalid MAC address: {mac}"));
  }

  let mut mac_bytes = Vec::with_capacity(6);
  for pair in cleaned.as_bytes().chunks(2) {
    let chunk = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
    mac_bytes.push(u8::from_str_radix(chunk, 16).map_err(|error| error.to_string())?);
  }

  let mut packet = vec![0xFF; 6];
  for _ in 0..16 {
    packet.extend_from_slice(&mac_bytes);
  }
  Ok(packet)
}

fn execute_poweroff(command: &str) -> Result<(), String> {
  if command.trim().is_empty() {
    return Err("当前未配置关机命令。".into());
  }

  #[cfg(target_os = "windows")]
  let status = Command::new("cmd").args(["/C", command]).status().map_err(|error| error.to_string())?;
  #[cfg(not(target_os = "windows"))]
  let status = Command::new("sh").args(["-c", command]).status().map_err(|error| error.to_string())?;

  if status.success() {
    Ok(())
  } else {
    Err(format!("关机命令执行返回状态异常: {status}"))
  }
}

fn normalize_controller_base(input: &str) -> String {
  let trimmed = input.trim();
  if trimmed.is_empty() {
    return String::new();
  }

  if let Some((scheme, rest)) = trimmed.split_once("://") {
    let normalized_scheme = match scheme {
      "http" => "ws",
      "https" => "wss",
      other => other,
    };
    let host = rest.split('/').next().unwrap_or(rest);
    return format!("{normalized_scheme}://{host}");
  }

  let host = trimmed.split('/').next().unwrap_or(trimmed);
  format!("ws://{host}")
}

fn normalize_ws_url(input: &str) -> String {
  let base = normalize_controller_base(input);
  if base.is_empty() {
    String::new()
  } else {
    format!("{}/api/wol/service", base.trim_end_matches('/'))
  }
}

fn config_dir() -> Result<PathBuf, String> {
  let base = dirs::config_dir().ok_or_else(|| "无法定位本地配置目录。".to_string())?;
  Ok(base.join(CONFIG_FOLDER))
}

fn config_path() -> Result<PathBuf, String> {
  Ok(config_dir()?.join(CONFIG_FILE))
}

fn load_config() -> AppConfig {
  let path = match config_path() {
    Ok(path) => path,
    Err(_) => return AppConfig::default(),
  };

  let mut config = fs::read_to_string(path)
    .ok()
    .and_then(|raw| serde_json::from_str::<AppConfig>(&raw).ok())
    .unwrap_or_default();
  config.message_key = "lucky666".into();
  config.execution_action = normalize_execution_action(&config.execution_action);
  if config.execution_action.is_empty() {
    config.execution_action = detect_execution_action_from_command(&config.power_off_cmd);
  }
  config.power_off_cmd = power_command_for_action(&config.execution_action);
  config
}

fn persist_config(config: &AppConfig) -> Result<(), String> {
  let directory = config_dir()?;
  fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
  let payload = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
  fs::write(directory.join(CONFIG_FILE), payload).map_err(|error| error.to_string())
}

fn default_device_name() -> String {
  get_hostname()
    .ok()
    .and_then(|value| value.into_string().ok())
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "Lucky被控端设备".into())
}

fn default_mac_address() -> String {
  get_mac_address()
    .ok()
    .flatten()
    .map(|mac| mac.to_string())
    .unwrap_or_default()
}

fn default_poweroff_cmd() -> String {
  power_command_for_action("shutdown")
}

fn normalize_execution_action(raw: &str) -> String {
  match raw.trim().to_ascii_lowercase().as_str() {
    "hibernate" => "hibernate".into(),
    "sleep" => "sleep".into(),
    _ => "shutdown".into(),
  }
}

fn detect_execution_action_from_command(command: &str) -> String {
  let lower = command.trim().to_ascii_lowercase();
  if lower.contains("shutdown /h") || lower.contains("hibernate") {
    "hibernate".into()
  } else if lower.contains("suspend") || lower.contains("sleepnow") || lower.contains("setsuspendstate") {
    "sleep".into()
  } else {
    "shutdown".into()
  }
}

fn power_command_for_action(action: &str) -> String {
  match normalize_execution_action(action).as_str() {
    "hibernate" => default_hibernate_cmd(),
    "sleep" => default_sleep_cmd(),
    _ => default_shutdown_cmd(),
  }
}

fn default_shutdown_cmd() -> String {
  #[cfg(target_os = "windows")]
  {
    "shutdown /s /t 0".into()
  }
  #[cfg(target_os = "linux")]
  {
    "shutdown -h now".into()
  }
  #[cfg(target_os = "macos")]
  {
    "osascript -e 'tell app \"System Events\" to shut down'".into()
  }
}

fn default_hibernate_cmd() -> String {
  #[cfg(target_os = "windows")]
  {
    "shutdown /h".into()
  }
  #[cfg(target_os = "linux")]
  {
    "systemctl hibernate".into()
  }
  #[cfg(target_os = "macos")]
  {
    "pmset sleepnow".into()
  }
}

fn default_sleep_cmd() -> String {
  #[cfg(target_os = "windows")]
  {
    "powershell -NoProfile -Command \"Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Application]::SetSuspendState('Suspend',$false,$false)\"".into()
  }
  #[cfg(target_os = "linux")]
  {
    "systemctl suspend".into()
  }
  #[cfg(target_os = "macos")]
  {
    "pmset sleepnow".into()
  }
}

fn unix_now() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs() as i64
}

fn unix_now_ms() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as i64
}

fn show_window(window: &WebviewWindow) {
  let _ = window.set_skip_taskbar(false);
  let _ = window.show();
  let _ = window.unminimize();
  let _ = window.set_focus();
}

fn hide_window(window: &WebviewWindow) {
  let _ = window.set_skip_taskbar(true);
  let _ = window.hide();
}

fn show_main_window(app: &AppHandle) {
  if let Some(window) = app.get_webview_window("main") {
    show_window(&window);
  }
}

fn build_tray(app: &AppHandle, _state: &SharedState) -> tauri::Result<()> {
  let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
  let quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?;
  let menu = Menu::with_items(app, &[&show, &quit])?;

  let show_handle = app.clone();

  let mut tray_builder = TrayIconBuilder::with_id("main-tray")
    .menu(&menu)
    .tooltip("Lucky WOL Agent")
    .show_menu_on_left_click(false)
    .on_tray_icon_event(move |_tray, event| {
      if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
      } = event
      {
        show_main_window(&show_handle);
      }
    })
    .on_menu_event(move |app, event: MenuEvent| match event.id().as_ref() {
      "show" => show_main_window(app),
      "quit" => app.exit(0),
      _ => {}
    });

  if let Some(icon) = app.default_window_icon().cloned() {
    tray_builder = tray_builder.icon(icon);
  }

  tray_builder.build(app)?;

  Ok(())
}

fn handle_window_event(window: &WebviewWindow, event: &WindowEvent, state: &SharedState) {
  match event {
    WindowEvent::CloseRequested { api, .. } => {
      if state.config_snapshot().minimize_to_tray {
        api.prevent_close();
        hide_window(window);
        state.push_event(EventLevel::Info, "窗口已隐藏到托盘", "程序仍会继续在后台运行。");
      }
    }
    WindowEvent::Resized(_) => {
      if state.config_snapshot().minimize_to_tray && window.is_minimized().unwrap_or(false) {
        hide_window(window);
      }
    }
    _ => {}
  }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(
      tauri_plugin_log::Builder::default()
        .level(log::LevelFilter::Info)
        .build(),
    )
    .plugin(tauri_plugin_autostart::init(
      MacosLauncher::LaunchAgent,
      None::<Vec<&str>>,
    ))
    .setup(|app| {
      let config = load_config();
      let state = SharedState::new(app.handle().clone(), config.clone());
      build_tray(app.handle(), &state)?;
      app.manage(state.clone());

      if let Some(window) = app.get_webview_window("main") {
        let watcher = state.clone();
        let managed_window = window.clone();
        window.on_window_event(move |event| {
          handle_window_event(&managed_window, &event, &watcher);
        });
        hide_window(&window);
      }

      state.push_event(EventLevel::Info, "程序已就绪", "桌面服务和本地运行环境已经初始化完成。");

      if config.launch_at_startup {
        let _ = app.handle().autolaunch().enable();
      }

      if config.auto_connect && !config.host.is_empty() && !config.token.is_empty() {
        state.start_connector();
      }

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      get_bootstrap,
      save_config,
      connect_now,
      reconnect_now,
      disconnect_now,
      cancel_pending_shutdown,
      execute_pending_shutdown_now,
      set_launch_at_startup
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
