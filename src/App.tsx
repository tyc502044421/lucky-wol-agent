import { useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  Cable,
  CheckCircle2,
  Cpu,
  Eye,
  EyeOff,
  HardDriveDownload,
  House,
  LaptopMinimal,
  LoaderCircle,
  Network,
  Power,
  RadioTower,
  RefreshCcw,
  Save,
  Settings,
  ShieldCheck,
  Sparkles,
  TimerReset,
  WifiOff,
} from 'lucide-react'
import clsx from 'clsx'
import './App.css'

type ExecutionAction = 'shutdown' | 'hibernate' | 'sleep'

type ToastLevel = 'success' | 'error' | 'info' | 'warning'

type Toast = {
  id: string
  level: ToastLevel
  message: string
}

type NetworkInterfaceInfo = {
  name: string
  mac: string
  ip: string | null
}

type AppConfig = {
  host: string
  token: string
  messageKey: string
  skipCertVerification: boolean
  executionAction: ExecutionAction
  deviceKey: string
  deviceName: string
  mac: string
  broadcastIp: string
  relay: boolean
  wolPort: number
  wolRepeat: number
  powerOffCmd: string
  autoConnect: boolean
  minimizeToTray: boolean
  launchAtStartup: boolean
  updateTime: number
}

type ConnectionSnapshot = {
  phase: 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'disconnected' | 'error'
  connected: boolean
  detail: string
  endpoint: string
  attempts: number
  lastError: string | null
  lastEventAt: string | null
}

type ActivityEvent = {
  level: 'info' | 'success' | 'warning' | 'error'
  title: string
  detail: string
  timestamp: string
}

type ShutdownPromptPayload = {
  deadlineUnixMs: number
  durationSeconds: number
}

type BootstrapPayload = {
  config: AppConfig
  connection: ConnectionSnapshot
  pendingShutdown: ShutdownPromptPayload | null
  autostartEnabled: boolean
  recentEvents: ActivityEvent[]
  version: string
}

const text = {
  waiting: '等待配置',
  idle: '待机',
  connecting: '连接中',
  connected: '已连接',
  reconnecting: '重连中',
  disconnected: '离线',
  error: '异常',
  autoConnect: '自动连接',
  tray: '托盘常驻',
  startup: '开机自启',
  executionAction: '自定义执行',
  actionShutdown: '关机',
  actionHibernate: '休眠',
  actionSleep: '睡眠',
  enabled: '已开启',
  disabled: '已关闭',
  manual: '手动',
  saved: '配置已保存，当前运行设置已刷新。',
  loadingTitle: '正在初始化 Lucky WOL 受控端',
  loadingBody: '正在加载本地配置和后台连接服务。',
  eyebrow: '第三方开源受控端',
  title: 'Lucky WOL 第三方受控端',
  subtitle: '适配 Lucky 网络唤醒主控端的第三方轻量受控端服务，保持与主控端的连接并响应远程指令。',
  emptyEndpoint: '尚未配置主控地址',
  connectNow: '立即连接',
  reconnectNow: '立即重连',
  disconnectNow: '断开连接',
  save: '保存配置',
  hostLabel: '主控地址',
  hostDesc: '支持 ws、wss、http、https 或直接填写 host:port。',
  tokenLabel: 'Token',
  tokenDesc: '需要与 Lucky 主控端中的 Token 保持一致。',
  deviceNameLabel: '设备名称',
  deviceNameDesc: '显示在 Lucky 主控界面中的设备名称。',
  broadcastLabel: '广播地址',
  broadcastDesc: '用于 Lucky 主控端记录当前设备的局域网广播地址。',
  defaultDevice: '未命名设备',
  defaultMac: '未识别 MAC 地址',
  mergedCard: '连接与设备设置',
  cancelAction: '取消执行',
  executeAction: '立即执行',
  seconds: '秒',
  macLabel: 'MAC 地址',
  macDesc: '本机网卡 MAC 地址，Wake on LAN 功能必须填写。',
  macPickerTitle: '选择网卡',
  macPickerDesc: '以下为当前设备检测到的所有网卡，点击选择对应 MAC 地址。',
  macPickerEmpty: '未检测到可用网卡',
  macPickerLoading: '正在读取网卡列表…',
  macPickerCancel: '取消',
  settingsTitle: '连接 Token 设置',
  settingsDesc: '请设置连接 Token，确认后需要点击「保存配置」才会生效。',
  settingsTokenLabel: '连接 Token',
  settingsTokenDesc: '需要与 Lucky 主控端中的连接 Token 保持一致。',
  settingsTokenShow: '显示',
  settingsTokenHide: '隐藏',
  settingsConfirm: '确认',
  settingsCancel: '取消',
}

const emptyBootstrap: BootstrapPayload = {
  config: {
    host: '',
    token: '',
    messageKey: 'lucky666',
    skipCertVerification: false,
    executionAction: 'shutdown',
    deviceKey: '',
    deviceName: '',
    mac: '',
    broadcastIp: '',
    relay: true,
    wolPort: 9,
    wolRepeat: 5,
    powerOffCmd: 'shutdown /s /t 0',
    autoConnect: true,
    minimizeToTray: true,
    launchAtStartup: false,
    updateTime: 0,
  },
  connection: {
    phase: 'idle',
    connected: false,
    detail: text.waiting,
    endpoint: '',
    attempts: 0,
    lastError: null,
    lastEventAt: null,
  },
  pendingShutdown: null,
  autostartEnabled: false,
  recentEvents: [],
  version: '0.0.0',
}

const phaseMeta: Record<ConnectionSnapshot['phase'], { label: string; tone: string; icon: typeof Cable }> = {
  idle: { label: text.idle, tone: 'muted', icon: TimerReset },
  connecting: { label: text.connecting, tone: 'warn', icon: LoaderCircle },
  connected: { label: text.connected, tone: 'good', icon: CheckCircle2 },
  reconnecting: { label: text.reconnecting, tone: 'warn', icon: RefreshCcw },
  disconnected: { label: text.disconnected, tone: 'muted', icon: WifiOff },
  error: { label: text.error, tone: 'danger', icon: ShieldCheck },
}

function App() {
  const [bootstrap, setBootstrap] = useState<BootstrapPayload>(emptyBootstrap)
  const [draft, setDraft] = useState<AppConfig>(emptyBootstrap.config)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const [toasts, setToasts] = useState<Toast[]>([])
  const [showMacPicker, setShowMacPicker] = useState(false)
  const [macInterfaces, setMacInterfaces] = useState<NetworkInterfaceInfo[]>([])
  const [macPickerLoading, setMacPickerLoading] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [settingsToken, setSettingsToken] = useState('')
  const [showSettingsToken, setShowSettingsToken] = useState(false)
  const [pendingShutdown, setPendingShutdown] = useState<ShutdownPromptPayload | null>(null)
  const [countdownNow, setCountdownNow] = useState(() => Date.now())
  const toastCounterRef = useRef(0)

  const addToast = useCallback((level: ToastLevel, message: string) => {
    const id = `toast-${++toastCounterRef.current}`
    setToasts((prev) => [...prev, { id, level, message }])
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id))
    }, 4000)
  }, [])

  const dismissToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id))
  }, [])

  const openMacPicker = async () => {
    setShowMacPicker(true)
    setMacPickerLoading(true)
    try {
      const list = await invoke<NetworkInterfaceInfo[]>('get_network_interfaces')
      setMacInterfaces(list)
    } catch (error) {
      addToast('error', String(error))
      setShowMacPicker(false)
    } finally {
      setMacPickerLoading(false)
    }
  }

  const selectMac = (mac: string) => {
    updateDraft('mac', mac)
    setShowMacPicker(false)
  }

  const openSettings = () => {
    setSettingsToken(draft.token)
    setShowSettingsToken(false)
    setShowSettings(true)
  }

  const applySettings = () => {
    updateDraft('token', settingsToken)
    setShowSettings(false)
  }

  const loadBootstrap = async () => {
    const payload = await invoke<BootstrapPayload>('get_bootstrap')
    setBootstrap(payload)
    setDraft(payload.config)
    setPendingShutdown(payload.pendingShutdown)
  }

  useEffect(() => {
    let cancelled = false
    const unlisteners: Array<() => void> = []

    ;(async () => {
      try {
        await loadBootstrap()
      } finally {
        if (!cancelled) setLoading(false)
      }

      const unlistenConnection = await listen<ConnectionSnapshot>('agent://connection-updated', (event) => {
        setBootstrap((prev) => ({ ...prev, connection: event.payload }))
      })
      const unlistenShutdownPending = await listen<ShutdownPromptPayload>('agent://shutdown-pending', (event) => {
        setPendingShutdown(event.payload)
      })
      const unlistenShutdownCleared = await listen('agent://shutdown-cleared', () => {
        setPendingShutdown(null)
      })

      unlisteners.push(unlistenConnection, unlistenShutdownPending, unlistenShutdownCleared)
    })()

    return () => {
      cancelled = true
      unlisteners.forEach((fn) => fn())
    }
  }, [])

  useEffect(() => {
    if (!pendingShutdown) return
    const timer = window.setInterval(() => setCountdownNow(Date.now()), 250)
    return () => window.clearInterval(timer)
  }, [pendingShutdown])

  const phase = phaseMeta[bootstrap.connection.phase]
  const PhaseIcon = phase.icon
  const dirty = JSON.stringify(draft) !== JSON.stringify(bootstrap.config)
  const shutdownSecondsLeft = pendingShutdown ? Math.max(0, Math.ceil((pendingShutdown.deadlineUnixMs - countdownNow) / 1000)) : 0


  const executionActionLabel =
    bootstrap.config.executionAction === 'hibernate'
      ? text.actionHibernate
      : bootstrap.config.executionAction === 'sleep'
        ? text.actionSleep
        : text.actionShutdown

  const executionActionVerb =
    bootstrap.config.executionAction === 'hibernate'
      ? '休眠'
      : bootstrap.config.executionAction === 'sleep'
        ? '睡眠'
        : '关机'

  const shutdownDialogTitle = `远程${executionActionVerb}确认`
  const shutdownDialogBody = `Lucky 主控端刚刚下发了${executionActionVerb}指令。你可以取消本次执行，也可以立即执行。`
  const shutdownDialogExecute = `立即${executionActionVerb}`
  const shutdownDialogCancel = `取消${executionActionVerb}`

  const updateDraft = <K extends keyof AppConfig>(key: K, value: AppConfig[K]) => {
    setDraft((current) => ({ ...current, [key]: value }))
  }

  const saveConfig = async () => {
    setSaving(true)
    try {
      const next = await invoke<BootstrapPayload>('save_config', { config: draft })
      setBootstrap(next)
      setDraft(next.config)
      addToast('success', text.saved)
    } catch (error) {
      addToast('error', String(error))
    } finally {
      setSaving(false)
    }
  }

  const runAction = async (action: 'connect_now' | 'disconnect_now' | 'reconnect_now') => {
    setBusyAction(action)
    try {
      await invoke(action)
      await loadBootstrap()
    } catch (error) {
      addToast('error', String(error))
    } finally {
      setBusyAction(null)
    }
  }

  const toggleAutostart = async () => {
    setBusyAction('autostart')
    try {
      const enabled = await invoke<boolean>('set_launch_at_startup', {
        enabled: !bootstrap.autostartEnabled,
      })
      setBootstrap((prev) => ({
        ...prev,
        autostartEnabled: enabled,
        config: { ...prev.config, launchAtStartup: enabled },
      }))
      setDraft((prev) => ({ ...prev, launchAtStartup: enabled }))
    } catch (error) {
      addToast('error', String(error))
    } finally {
      setBusyAction(null)
    }
  }

  const handleCancelShutdown = async () => {
    setBusyAction('cancel_shutdown')
    try {
      await invoke('cancel_pending_shutdown')
      setPendingShutdown(null)
    } catch (error) {
      addToast('error', String(error))
    } finally {
      setBusyAction(null)
    }
  }

  const handleExecuteShutdownNow = async () => {
    setBusyAction('execute_shutdown_now')
    try {
      await invoke('execute_pending_shutdown_now')
      setPendingShutdown(null)
    } catch (error) {
      addToast('error', String(error))
    } finally {
      setBusyAction(null)
    }
  }

  if (loading) {
    return (
      <main className="shell shell--loading">
        <div className="loading-panel">
          <LoaderCircle className="spin" size={26} />
          <div>
            <strong>{text.loadingTitle}</strong>
            <p>{text.loadingBody}</p>
          </div>
        </div>
      </main>
    )
  }

  return (
    <main className="shell">
      <div className="ambient ambient--one" />
      <div className="ambient ambient--two" />

      <header className="hero-panel">
        <div className="hero-copy">
          <span className="eyebrow">
            <Sparkles size={13} />
            {text.eyebrow}
          </span>
          <h1>{text.title}</h1>
          <p>{text.subtitle}</p>

          <div className="hero-device-summary">
            <article className="summary-chip">
              <LaptopMinimal size={16} />
              <div>
                <span>设备名称</span>
                <strong>{draft.deviceName || text.defaultDevice}</strong>
              </div>
            </article>
            <article className="summary-chip">
              <Cpu size={16} />
              <div>
                <span>MAC 地址</span>
                <strong>{draft.mac || text.defaultMac}</strong>
              </div>
            </article>
          </div>
        </div>

        <div className="hero-status">
          <div className={clsx('status-pill', `status-pill--${phase.tone}`)}>
            <PhaseIcon size={15} className={bootstrap.connection.phase === 'connecting' ? 'spin' : undefined} />
            <span>{phase.label}</span>
          </div>

          <div className="status-body">
            <strong>{bootstrap.connection.detail}</strong>
            <span>{bootstrap.connection.endpoint || text.emptyEndpoint}</span>
          </div>

          <div className="hero-actions">
            <button className="button button--secondary" onClick={() => runAction('connect_now')} disabled={busyAction !== null}>
              <Cable size={15} className={busyAction === 'connect_now' ? 'spin' : undefined} />
              {text.connectNow}
            </button>
            <button className="button button--ghost" onClick={() => runAction('reconnect_now')} disabled={busyAction !== null}>
              <RefreshCcw size={15} className={busyAction === 'reconnect_now' ? 'spin' : undefined} />
              {text.reconnectNow}
            </button>
            <button className="button button--secondary" onClick={() => runAction('disconnect_now')} disabled={busyAction !== null}>
              <WifiOff size={15} className={busyAction === 'disconnect_now' ? 'spin' : undefined} />
              {text.disconnectNow}
            </button>
            <button className="button button--primary" onClick={saveConfig} disabled={!dirty || saving}>
              <Save size={15} className={saving ? 'spin' : undefined} />
              {text.save}
            </button>
          </div>
        </div>
      </header>

      <section className="control-strip">
        <article className="metric-card metric-card--merged">
          <div className="metric-card__item">
            <RadioTower size={15} />
            <div>
              <span>{text.autoConnect}</span>
              <strong>{draft.autoConnect ? text.enabled : text.manual}</strong>
            </div>
          </div>
          <div className="metric-card__divider" />
          <div className="metric-card__item">
            <House size={15} />
            <div>
              <span>{text.tray}</span>
              <strong>{draft.minimizeToTray ? text.enabled : text.disabled}</strong>
            </div>
          </div>
        </article>

        <label className="select-card">
          <div className="select-card__meta">
            <Power size={15} />
            <div>
              <span>{text.executionAction}</span>
              <strong>{executionActionLabel}</strong>
            </div>
          </div>
          <select value={draft.executionAction} onChange={(event) => updateDraft('executionAction', event.target.value as ExecutionAction)}>
            <option value="shutdown">{text.actionShutdown}</option>
            <option value="hibernate">{text.actionHibernate}</option>
            <option value="sleep">{text.actionSleep}</option>
          </select>
        </label>

        <button className="toggle-card" onClick={toggleAutostart} disabled={busyAction !== null} type="button">
          <div className="toggle-card__meta">
            <HardDriveDownload size={15} className={busyAction === 'autostart' ? 'spin' : undefined} />
            <div>
              <span>{text.startup}</span>
              <strong>{bootstrap.autostartEnabled ? text.enabled : text.disabled}</strong>
            </div>
          </div>
          <span className={clsx('toggle', bootstrap.autostartEnabled && 'toggle--checked')}>
            <span />
          </span>
        </button>

        <button
          className="control-strip__settings-btn"
          onClick={openSettings}
          title={text.settingsTitle}
          type="button"
        >
          <Settings size={16} />
        </button>
      </section>

      <section className="workspace-grid workspace-grid--compact">
        <section className="panel form-card form-card--merged">
          <div className="panel-head">
            <span>{text.mergedCard}</span>
          </div>

          <div className="merged-form-grid">
            <LabeledField label={text.hostLabel} description={text.hostDesc}>
              <input value={draft.host} onChange={(event) => updateDraft('host', event.target.value)} placeholder="192.168.1.3:16601" />
            </LabeledField>
            <LabeledField label={text.deviceNameLabel} description={text.deviceNameDesc}>
              <input value={draft.deviceName} onChange={(event) => updateDraft('deviceName', event.target.value)} placeholder="办公电脑" />
            </LabeledField>
            <LabeledField label={text.broadcastLabel} description={text.broadcastDesc}>
              <input value={draft.broadcastIp} onChange={(event) => updateDraft('broadcastIp', event.target.value)} placeholder="192.168.1.255" />
            </LabeledField>
            <LabeledField label={text.macLabel} description={text.macDesc}>
              <div className="field-with-action">
                <input value={draft.mac} onChange={(event) => updateDraft('mac', event.target.value)} placeholder="AA:BB:CC:DD:EE:FF" />
                <button className="field-action-btn" onClick={openMacPicker} type="button" title="从网卡列表中选择">
                  <Network size={14} />
                </button>
              </div>
            </LabeledField>
          </div>
        </section>
      </section>

      <ToastContainer toasts={toasts} onDismiss={dismissToast} />

      {showMacPicker && (
        <MacPickerDialog
          loading={macPickerLoading}
          interfaces={macInterfaces}
          onSelect={selectMac}
          onClose={() => setShowMacPicker(false)}
        />
      )}

      {showSettings && (
        <SettingsDialog
          token={settingsToken}
          showToken={showSettingsToken}
          onTokenChange={setSettingsToken}
          onToggleShowToken={() => setShowSettingsToken((v) => !v)}
          onConfirm={applySettings}
          onClose={() => setShowSettings(false)}
        />
      )}

      {pendingShutdown ? (
        <div className="shutdown-overlay">
          <div className="shutdown-dialog">
            <div className="shutdown-dialog__badge">
              <Power size={18} />
              {shutdownDialogTitle}
            </div>
            <h2>
              {shutdownSecondsLeft} {text.seconds}
            </h2>
            <p>{shutdownDialogBody}</p>
            <div className="shutdown-dialog__countdown">
              <strong>{shutdownSecondsLeft}</strong>
              <span>{text.seconds}</span>
            </div>
            <div className="shutdown-dialog__actions">
              <button className="button button--ghost" onClick={handleCancelShutdown} disabled={busyAction !== null}>
                {shutdownDialogCancel}
              </button>
              <button className="button button--danger" onClick={handleExecuteShutdownNow} disabled={busyAction !== null}>
                {shutdownDialogExecute}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </main>
  )
}

type LabeledFieldProps = {
  label: string
  description: string
  children: ReactNode
}

function LabeledField({ label, description, children }: LabeledFieldProps) {
  return (
    <label className="field">
      <span className="field__meta">
        <strong>{label}</strong>
        <small>{description}</small>
      </span>
      {children}
    </label>
  )
}

type SettingsDialogProps = {
  token: string
  showToken: boolean
  onTokenChange: (value: string) => void
  onToggleShowToken: () => void
  onConfirm: () => void
  onClose: () => void
}

function SettingsDialog({ token, showToken, onTokenChange, onToggleShowToken, onConfirm, onClose }: SettingsDialogProps) {
  return (
    <div className="settings-overlay" onClick={onClose}>
      <div className="settings-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="settings-dialog__head">
          <div className="settings-dialog__title">
            <Settings size={15} />
            <span>{text.settingsTitle}</span>
          </div>
          <button className="mac-picker-dialog__close" onClick={onClose} aria-label="关闭">✕</button>
        </div>
        <p className="settings-dialog__desc">{text.settingsDesc}</p>

        <label className="field">
          <span className="field__meta">
            <strong>{text.settingsTokenLabel}</strong>
            <small>{text.settingsTokenDesc}</small>
          </span>
          <div className="field-with-action">
            <input
              type={showToken ? 'text' : 'password'}
              value={token}
              onChange={(e) => onTokenChange(e.target.value)}
              placeholder="请输入 Token"
              autoComplete="off"
            />
            <button className="field-action-btn" onClick={onToggleShowToken} type="button" title={showToken ? text.settingsTokenHide : text.settingsTokenShow}>
              {showToken ? <EyeOff size={14} /> : <Eye size={14} />}
            </button>
          </div>
        </label>

        <div className="settings-dialog__footer">
          <button className="button button--ghost" onClick={onClose}>{text.settingsCancel}</button>
          <button className="button button--primary" onClick={onConfirm}>{text.settingsConfirm}</button>
        </div>
      </div>
    </div>
  )
}

type MacPickerDialogProps = {
  loading: boolean
  interfaces: NetworkInterfaceInfo[]
  onSelect: (mac: string) => void
  onClose: () => void
}

function MacPickerDialog({ loading, interfaces, onSelect, onClose }: MacPickerDialogProps) {
  return (
    <div className="mac-picker-overlay" onClick={onClose}>
      <div className="mac-picker-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="mac-picker-dialog__head">
          <div className="mac-picker-dialog__title">
            <Network size={15} />
            <span>{text.macPickerTitle}</span>
          </div>
          <button className="mac-picker-dialog__close" onClick={onClose} aria-label="关闭">✕</button>
        </div>
        <p className="mac-picker-dialog__desc">{text.macPickerDesc}</p>
        <div className="mac-picker-list">
          {loading ? (
            <div className="mac-picker-empty">
              <LoaderCircle size={18} className="spin" />
              <span>{text.macPickerLoading}</span>
            </div>
          ) : interfaces.length === 0 ? (
            <div className="mac-picker-empty">
              <span>{text.macPickerEmpty}</span>
            </div>
          ) : (
            interfaces.map((iface) => (
              <button key={iface.mac} className="mac-picker-item" onClick={() => onSelect(iface.mac)} type="button">
                <span className="mac-picker-item__name">
                  {iface.name}
                  {iface.ip && <span className="mac-picker-item__ip">{iface.ip}</span>}
                </span>
                <span className="mac-picker-item__mac">{iface.mac}</span>
              </button>
            ))
          )}
        </div>
        <div className="mac-picker-dialog__footer">
          <button className="button button--ghost" onClick={onClose}>{text.macPickerCancel}</button>
        </div>
      </div>
    </div>
  )
}

type ToastContainerProps = {
  toasts: Toast[]
  onDismiss: (id: string) => void
}

function ToastContainer({ toasts, onDismiss }: ToastContainerProps) {
  if (toasts.length === 0) return null
  return (
    <div className="toast-container">
      {toasts.map((toast) => (
        <div key={toast.id} className={clsx('toast', `toast--${toast.level}`)}>
          <span className="toast__message">{toast.message}</span>
          <button className="toast__close" onClick={() => onDismiss(toast.id)} aria-label="关闭">✕</button>
        </div>
      ))}
    </div>
  )
}

export default App
