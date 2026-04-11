/** @type {WebSocket | null} */
let ws = null;
let listeners = [];

export function connect(url = 'ws://localhost:3030') {
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
      // event.data may be a Blob in some browsers
      const raw = typeof event.data === 'string' ? event.data : null;
      if (!raw) {
        // Handle Blob data
        event.data.text().then((text) => {
          const data = JSON.parse(text);
          console.log('[ws] event:', data.type);
          notify(data);
        });
        return;
      }
      const data = JSON.parse(raw);
      console.log('[ws] event:', data.type);
      notify(data);
    } catch (e) {
      console.error('[ws] parse error:', e, event.data);
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
