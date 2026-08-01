(async function () {
  const prompt = "__PROMPT__";

  const logs = [];
  function log(level, message) {
    const line = `[${level}] ${message}`;
    logs.push(line);
    // eslint-disable-next-line no-console
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
    // The Quill clipboard element is contenteditable but must not be used as the input.
    if (!el) return true;
    const cls = (el.className || '').toString();
    if (cls.includes('ql-clipboard')) return true;
    const ariaLabel = (el.getAttribute('aria-label') || '').toLowerCase();
    if (ariaLabel && ariaLabel.includes('clipboard')) return true;
    return false;
  }

  function findInput() {
    // Prefer the specific Quill editor that the Gemini UI currently uses.
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

  async function waitForInput(maxMs) {
    const deadline = Date.now() + maxMs;
    while (Date.now() < deadline) {
      const el = findInput();
      if (el && isFocusable(el)) {
        return el;
      }
      if (el && !isVisible(el)) {
        log('info', 'input found but not visible yet, polling...');
      }
      await sleep(200);
    }
    return null;
  }

  function setValue(el, text) {
    if (!el) return;
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      el.focus();
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

  function dispatchInput(el, type) {
    // Use InputEvent when available for richer event metadata.
    if (typeof InputEvent !== 'undefined') {
      const event = new InputEvent(type, {
        bubbles: true,
        cancelable: true,
        inputType: 'insertText',
        data: prompt,
      });
      el.dispatchEvent(event);
    } else {
      dispatch(el, type, { inputType: 'insertText' });
    }
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

  // Wait up to 10 seconds for the input to appear and become usable.
  log('info', 'waiting for Gemini input element...');
  const input = await waitForInput(10000);
  if (!input) {
    log('error', 'could not find a visible, focusable Gemini input element');
    log('info', `document readyState: ${document.readyState}`);
    log('info', `total contenteditable elements: ${document.querySelectorAll('[contenteditable="true"]').length}`);
    log('info', `total role=textbox elements: ${document.querySelectorAll('[role="textbox"]').length}`);
    log('info', `total textarea elements: ${document.querySelectorAll('textarea').length}`);
    log('info', `location: ${location.href}`);
    window.__geminiSimLogs = logs;
    return false;
  }

  log('info', `input found: ${input.tagName} class="${input.className}" aria-label="${input.getAttribute('aria-label') || ''}"`);

  input.focus();
  input.click();
  setValue(input, prompt);

  dispatchInput(input, 'input');
  dispatch(input, 'change');
  await sleep(50);

  dispatchKeyboard(input, 'keydown', 'Enter');
  dispatchKeyboard(input, 'keypress', 'Enter');
  dispatchKeyboard(input, 'keyup', 'Enter');

  // Give the UI a moment to render the send button (if it appears after text entry).
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
