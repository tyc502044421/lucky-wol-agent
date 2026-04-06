import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  Cable,
  CheckCircle2,
  Cpu,
  HardDriveDownload,
  House,
  LaptopMinimal,
  LoaderCircle,
  Power,
  RadioTower,
  RefreshCcw,
  Save,
  ServerCog,
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

  const runtimeHints = useMemo(
    () => [
      { label: text.autoConnect, value: draft.autoConnect ? text.enabled : text.manual, icon: RadioTower },
      { label: text.tray, value: draft.minimizeToTray ? text.enabled : text.disabled, icon: House },
    ],
    [draft.autoConnect, draft.minimizeToTray],
  )

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
        {runtimeHints.map(({ icon: Icon, label, value }) => (
          <article className="metric-card" key={label}>
            <Icon size={15} />
            <div>
              <span>{label}</span>
              <strong>{value}</strong>
            </div>
          </article>
        ))}

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
      </section>

      <section className="workspace-grid workspace-grid--compact">
        <section className="panel form-card form-card--merged">
          <div className="panel-head">
            <span>{text.mergedCard}</span>
            <ServerCog size={15} />
          </div>

          <div className="merged-form-grid">
            <LabeledField label={text.hostLabel} description={text.hostDesc}>
              <input value={draft.host} onChange={(event) => updateDraft('host', event.target.value)} placeholder="192.168.1.3:16601" />
            </LabeledField>
            <LabeledField label={text.deviceNameLabel} description={text.deviceNameDesc}>
              <input value={draft.deviceName} onChange={(event) => updateDraft('deviceName', event.target.value)} placeholder="办公电脑" />
            </LabeledField>
            <LabeledField label={text.tokenLabel} description={text.tokenDesc}>
              <input value={draft.token} onChange={(event) => updateDraft('token', event.target.value)} placeholder="请输入 Token" />
            </LabeledField>
            <LabeledField label={text.broadcastLabel} description={text.broadcastDesc}>
              <input value={draft.broadcastIp} onChange={(event) => updateDraft('broadcastIp', event.target.value)} placeholder="192.168.1.255" />
            </LabeledField>
          </div>
        </section>
      </section>

      <ToastContainer toasts={toasts} onDismiss={dismissToast} />

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
