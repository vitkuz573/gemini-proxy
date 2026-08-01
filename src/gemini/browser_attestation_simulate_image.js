// Injected into the Gemini headless page to paste a local image and prompt.
// The caller replaces __IMAGE_PATH__ and __PROMPT__.
(async function () {
  function findSendButton() {
    const selectors = [
      'button[aria-label*="Send"]',
      'button[aria-label*="send"]',
      'button[data-test-id="send-button"]',
      'button.send-button',
      'button[aria-label="Send message"]',
      '[data-test-id="send-button"]',
      '[data-test-id="submit-button"]',
      'button[aria-label*="submit"]',
      'button[aria-label*="Submit"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el && isVisible(el) && !el.disabled) return el;
    }
    const buttons = Array.from(document.querySelectorAll('button'));
    return (
      buttons.find((b) => {
        if (!isVisible(b) || b.disabled) return false;
        const text = (b.textContent || b.getAttribute('aria-label') || '').toLowerCase();
        return text.includes('send') || text.includes('submit');
      }) || null
    );
  }

  function setValue(el, text) {
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      el.value = text;
    } else if (el.contentEditable === 'true' || el.getAttribute('role') === 'textbox') {
      const html = text ? `<p>${text.replace(/\n/g, '<br>')}</p>` : '<p><br></p>';
      el.focus();
      if (document.queryCommandSupported && document.queryCommandSupported('insertHTML')) {
        document.execCommand('selectAll', false, null);
        document.execCommand('insertHTML', false, html);
      } else {
        el.innerHTML = html;
      }
    } else if (el.shadowRoot) {
      const inner = el.shadowRoot.querySelector('textarea, input, [contenteditable="true"]');
      if (inner) setValue(inner, text);
    }
  }

  function dispatch(el, type, init) {
    const event = new Event(type, { bubbles: true, cancelable: true, ...init });
    el.dispatchEvent(event);
  }

  function dispatchKeyboard(el, type, key) {
    const event = new KeyboardEvent(type, {
      key,
      code: key === 'Enter' ? 'Enter' : key,
      keyCode: key === 'Enter' ? 13 : 0,
      which: key === 'Enter' ? 13 : 0,
      bubbles: true,
      cancelable: true,
    });
    el.dispatchEvent(event);
  }

  const logs = [];
  function log(level, message) {
    const line = `[${level}] ${message}`;
    logs.push(line);
    if (typeof console !== 'undefined' && console.log) console.log(line);
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function isVisible(el) {
    if (!el) return false;
    const rect = el.getBoundingClientRect && el.getBoundingClientRect();
    if (!rect) return false;
    return rect.width > 0 && rect.height > 0;
  }

  function isFocusable(el) {
    if (!el) return false;
    return typeof el.focus === 'function' && el.tabIndex !== -1 && isVisible(el);
  }

  function rejectClipboard(el) {
    if (!el) return true;
    const cls = (el.className || '').toString();
    if (cls.includes('ql-clipboard')) return true;
    const ariaLabel = (el.getAttribute('aria-label') || '').toLowerCase();
    if (ariaLabel && ariaLabel.includes('clipboard')) return true;
    return false;
  }

  function findInput() {
    const selectors = [
      '.ql-editor[contenteditable="true"][aria-label*="prompt"]',
      '.ql-editor[contenteditable="true"]',
      '[data-test-id="textarea-inner"] rich-textarea [contenteditable="true"]',
      '[data-test-id="textarea-wrapper"] rich-textarea [contenteditable="true"]',
      'rich-textarea [contenteditable="true"]',
      'rich-textarea',
      '[data-test-id="input-text"]',
      '[data-test-id="chat-input-textbox"]',
      '[aria-label*="prompt"][contenteditable="true"]',
      '[aria-label*="prompt"]',
      'textarea',
      'input[type="text"]',
      '[contenteditable="true"]',
      '[role="textbox"]',
    ];
    for (const s of selectors) {
      const nodes = Array.from(document.querySelectorAll(s));
      for (const el of nodes) {
        if (rejectClipboard(el)) continue;
        return el;
      }
    }
    return null;
  }

  async function waitForInput(maxMs) {
    const deadline = Date.now() + maxMs;
    while (Date.now() < deadline) {
      const el = findInput();
      if (el && isFocusable(el)) return el;
      await sleep(200);
    }
    return null;
  }

  log('info', 'waiting for Gemini input element...');
  const input = await waitForInput(10000);
  if (!input) {
    log('error', 'could not find a visible, focusable Gemini input element');
    window.__geminiSimLogs = logs;
    return false;
  }

  log('info', `input found: ${input.tagName} class="${input.className}"`);

  input.focus();
  input.click();

  // Load the local image via file:// so it appears as a clipboard paste.
  const imagePath = '__IMAGE_PATH__';
  try {
    const xhr = new XMLHttpRequest();
    xhr.open('GET', imagePath, false); // synchronous for simplicity
    xhr.responseType = 'blob';
    xhr.send();
    const blob = xhr.response;
    if (blob && blob.size > 0) {
      const filename = imagePath.split('/').pop();
      const file = new File([blob], filename, { type: blob.type || 'image/png' });
      const dt = new DataTransfer();
      dt.items.add(file);
      const pasteEvent = new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData: dt,
      });
      input.dispatchEvent(pasteEvent);
      // Dispatch on body as well to catch global paste handlers.
      document.body.dispatchEvent(pasteEvent);
      dispatch(input, 'input', { inputType: 'insertFromPaste' });
      dispatch(input, 'change');
    }
  } catch (e) {
    // Fall through to text-only path if image paste fails.
    log('warn', `image paste failed: ${e && e.message ? e.message : e}`);
  }

  setValue(input, '__PROMPT__');
  dispatch(input, 'input', { inputType: 'insertText' });
  dispatch(input, 'change');
  await sleep(50);
  dispatchKeyboard(input, 'keydown', 'Enter');
  dispatchKeyboard(input, 'keypress', 'Enter');
  dispatchKeyboard(input, 'keyup', 'Enter');

  await sleep(150);
  const btn = findSendButton();
  if (btn) {
    log('info', 'clicking send button');
    btn.focus();
    btn.click();
    dispatch(btn, 'mousedown');
    dispatch(btn, 'mouseup');
  } else {
    log('info', 'no send button found; relying on Enter key submission');
  }

  window.__geminiSimLogs = logs;
  return true;
})();
