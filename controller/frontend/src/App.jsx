import { useEffect, useState } from 'react'

// Reads from the bridge daemon's local WebSocket. When this page is served
// to the device's own on-device Chromium (CDP navigation to localhost), or
// when a dev machine has `adb forward tcp:8765 tcp:8765` active, "localhost"
// resolves correctly either way - no device IP needed.
const WS_URL = 'ws://localhost:8765'

function describe(msg) {
  if (msg.type === 'button') {
    const action = msg.value === 1 ? 'pressed' : msg.value === 0 ? 'released' : `value=${msg.value}`
    return `Button code ${msg.code} ${action}`
  }
  if (msg.type === 'knob') {
    return `Knob delta: ${msg.delta}`
  }
  return JSON.stringify(msg)
}

function App() {
  const [status, setStatus] = useState('connecting...')
  const [last, setLast] = useState(null)
  const [log, setLog] = useState([])

  useEffect(() => {
    let ws
    let cancelled = false

    function connect() {
      ws = new WebSocket(WS_URL)
      ws.onopen = () => !cancelled && setStatus('connected')
      ws.onclose = () => {
        if (cancelled) return
        setStatus('disconnected - retrying...')
        setTimeout(connect, 1000)
      }
      ws.onerror = () => !cancelled && setStatus('error - retrying...')
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data)
          const text = describe(msg)
          setLast(text)
          setLog((prev) => [text, ...prev].slice(0, 10))
        } catch (e) {
          // ignore non-JSON frames (e.g. pings surfaced as messages)
        }
      }
    }
    connect()

    return () => {
      cancelled = true
      ws && ws.close()
    }
  }, [])

  return (
    <div
      style={{
        fontFamily: 'monospace',
        background: '#000',
        color: '#0f0',
        height: '100vh',
        width: '100vw',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: '1rem',
      }}
    >
      <div style={{ fontSize: '0.9rem', opacity: 0.6 }}>bridge: {status}</div>
      <div style={{ fontSize: '2rem', textAlign: 'center', padding: '0 1rem' }}>
        {last ?? 'waiting for hardware input...'}
      </div>
      <ul style={{ fontSize: '0.8rem', opacity: 0.5, listStyle: 'none', padding: 0 }}>
        {log.slice(1).map((l, i) => (
          <li key={i}>{l}</li>
        ))}
      </ul>
    </div>
  )
}

export default App
