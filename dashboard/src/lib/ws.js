/** @type {WebSocket | null} */
let ws = null;
let listeners = [];

export function connect(url = 'ws://localhost:3001') {
  if (ws) ws.close();

  ws = new WebSocket(url);

  ws.onopen = () => {
    console.log('[ws] connected to', url);
    notify({ type: 'ws_connected' });
  };

  ws.onclose = () => {
    console.log('[ws] disconnected, reconnecting in 2s...');
    notify({ type: 'ws_disconnected' });
    setTimeout(() => connect(url), 2000);
  };

  ws.onerror = (e) => {
    console.error('[ws] error:', e);
  };

  ws.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      notify(data);
    } catch (e) {
      console.error('[ws] parse error:', e);
    }
  };
}

export function onEvent(fn) {
  listeners.push(fn);
  return () => {
    listeners = listeners.filter((l) => l !== fn);
  };
}

function notify(data) {
  for (const fn of listeners) fn(data);
}
