// Injected into the Gemini headless page to type a prompt and submit it.
// The caller replaces __PROMPT__ with the user prompt.
(async function () {
  const prompt = "__PROMPT__";

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
      // New simplified input area (August 2026): a plain textarea with the
      // placeholder "Ask Gemini" is rendered server-side.
      'textarea.gds-body-l[placeholder="Ask Gemini"]',
      'textarea[placeholder="Ask Gemini"]',
      '.initial-input-area textarea',
      // Legacy Quill-based rich textarea.
      '.ql-editor.textarea[contenteditable="true"]',
      '.ql-editor[contenteditable="true"][aria-label*="prompt"]',
      '.ql-editor[contenteditable="true"][aria-label*="Prompt"]',
      '.ql-editor[contenteditable="true"]',
      '[contenteditable="true"][aria-label*="prompt"]',
      '[contenteditable="true"][aria-label*="Prompt"]',
      '[data-test-id="textarea-inner"] rich-textarea [contenteditable="true"]',
      '[data-test-id="textarea-wrapper"] rich-textarea [contenteditable="true"]',
      'rich-textarea [contenteditable="true"]',
      'rich-textarea',
      '[data-test-id="input-text"]',
      '[data-test-id="chat-input-textbox"]',
      'textarea',
      'input[type="text"]',
      '[contenteditable="true"]',
      '[role="textbox"]',
    ];
    for (const s of selectors) {
      const nodes = Array.from(document.querySelectorAll(s));
      for (const el of nodes) {
        if (rejectClipboard(el)) continue;
        if (!isVisible(el)) continue;
        return el;
      }
    }
    return null;
  }

  function findSendButton() {
    const selectors = [
      // New simplified input area: the send icon is a <mat-icon> with
      // class "send-icon" inside the input container.
      '.initial-input-area .send-icon',
      '.initial-input-area-container .send-icon',
      '.initial-input-area mat-icon',
      // Legacy Angular send button wrappers.
      'button[aria-label*="Send"]',
      'button[aria-label*="send"]',
      'button[data-test-id="send-button"]',
      'button.send-button',
      'button[aria-label="Send message"]',
      '[data-test-id="send-button"]',
      '[data-test-id="submit-button"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el && isVisible(el)) return el;
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
      if (el) {
        return el;
      }
      await sleep(200);
    }
    return null;
  }

  log('info', 'waiting for Gemini input element...');
  const input = await waitForInput(15000);
  if (!input) {
    log('error', 'could not find a visible Gemini input element');
    log('info', `document readyState: ${document.readyState}`);
    log('info', `total contenteditable elements: ${document.querySelectorAll('[contenteditable="true"]').length}`);
    log('info', `total textarea elements: ${document.querySelectorAll('textarea').length}`);
    log('info', `location: ${location.href}`);
    window.__geminiSimLogs = logs;
    return false;
  }

  log('info', `input found: ${input.tagName} class="${input.className}" aria-label="${input.getAttribute('aria-label') || ''}"`);

  // Focus and click the input, then wait briefly so Angular attaches listeners.
  input.focus();
  input.click();
  await sleep(200);

  function setText(el, text) {
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      el.value = text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return el.value === text;
    }
    // Contenteditable / Quill editor.
    el.tabIndex = 0;
    el.focus();
    const sel = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
    document.execCommand('selectAll', false, null);
    const inserted = document.execCommand('insertText', false, text);
    if (!inserted || el.innerText.trim().length < text.length / 2) {
      // Fallback: set innerText directly and dispatch events.
      el.innerText = text;
      el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    }
    return el.innerText.trim().length >= text.length / 2;
  }

  const ok = setText(input, prompt);
  log('info', `text insertion result: ${ok}, innerText: "${(input.innerText || input.value || '').slice(0, 80)}"`);

  await sleep(150);

  // Try to click the real send button; this is the submission path the live
  // UI expects. If no button is visible, the caller will fall back to a real
  // Enter key via CDP.
  function findSendButton() {
    const selectors = [
      '.initial-input-area .send-icon',
      '.initial-input-area-container .send-icon',
      '.initial-input-area mat-icon',
      'button[aria-label*="Send"]',
      'button[aria-label*="send"]',
      'button[data-test-id="send-button"]',
      'button.send-button',
      'button[aria-label="Send message"]',
      '[data-test-id="send-button"]',
      '[data-test-id="submit-button"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el) {
        const rect = el.getBoundingClientRect && el.getBoundingClientRect();
        if (rect && rect.width > 0 && rect.height > 0) return el;
      }
    }
    const buttons = Array.from(document.querySelectorAll('button'));
    return (
      buttons.find((b) => {
        const rect = b.getBoundingClientRect && b.getBoundingClientRect();
        if (!rect || rect.width === 0 || rect.height === 0 || b.disabled) return false;
        const text = (b.textContent || b.getAttribute('aria-label') || '').toLowerCase();
        return text.includes('send') || text.includes('submit');
      }) || null
    );
  }

  await sleep(150);
  const sendBtn = findSendButton();
  let clickedSend = false;
  if (sendBtn) {
    log('info', `clicking send button: ${sendBtn.tagName}`);
    const gemBtn = sendBtn.closest('gem-icon-button');
    (gemBtn || sendBtn).click();
    clickedSend = true;
  } else {
    log('info', 'no send button found; relying on Enter fallback');
  }

  // Return the input's bounding rect so the caller can dispatch a real Enter
  // key as a fallback if the click did not submit.
  const rect = input.getBoundingClientRect();
  const result = {
    ok: true,
    tag: input.tagName,
    className: input.className,
    text: (input.innerText || input.value || '').slice(0, 200),
    clickedSend: clickedSend,
    rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
  };
  log('info', `returning input rect and text: ${JSON.stringify(result)}`);

  window.__geminiSimLogs = logs;
  return result;
})();
